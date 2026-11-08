use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use darksouls3::app_menu::NewMenuSystem;
use darksouls3::param::{
    EQUIP_PARAM_PROTECTOR_ST, EQUIP_PARAM_WEAPON_ST, NPC_PARAM_ST, NPC_THINK_PARAM_ST,
    REINFORCE_PARAM_WEAPON_ST,
};
use darksouls3::sprj::{
    CSRegulationManager, GameDataMan, ItemCategory, ItemId, MenuMan, PlayerGameData, PlayerInfo, PlayerIns,
    WorldChrMan,
};
use pelite::pe64::Pe;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use fromsoftware_shared::{FromStatic, Program};

use crate::pattern::{self, find_unique};

pub const HELD_SOULS_OFFSET: usize = 0x74;
pub const SOUL_LEVEL_OFFSET: usize = 0x70;
const SOUL_SCALE_BASE_LEVEL: i32 = 10;
pub const QUICK_ITEM_OFFSET: usize = 0x5AC;
pub const QUICK_ITEM_COUNT: usize = 10;
/// Player data index of the highlighted quick item. 0 was Estus, 1 was Ashen Estus
/// after a d-pad switch that changed nothing else in the block.
pub const SELECTED_QUICK_SLOT_OFFSET: usize = 0x4E0;
const ONLINE_FLAG_OFFSET: usize = 0xB1C;

/// `mov rax, [rip+disp32]; mov [rax+28h], ecx; ret` with the displacement's high byte fixed.
const GAME_MAN_PATTERN: &str = "48 8B 05 ?? ?? ?? 04 89 48 28 C3";
const ADD_SOUL_PATTERN: &str =
    "48 89 5C 24 ?? 48 89 74 24 ?? 57 48 83 EC ?? 8B DA 48 8B F9 E8 ?? ?? ?? ?? 8D 14 18";
/// Prologue of the function behind the event script's `SetTextEffect` command.
const SET_TEXT_EFFECT_PATTERN: &str =
    "40 57 48 83 EC 30 48 C7 44 24 20 FE FF FF FF 48 89 5C 24 40 0F B6 FA 8B D9 8D 41 FF 83 F8 15 0F 87";
/// The death grey's `SprjFade` call on plate 6: load the fade singleton, build the
/// colour and duration arguments, then call the timed fade setter.
const FADE_PATTERN: &str =
    "48 8B 0D ?? ?? ?? ?? 4C 8D 4C 24 28 4C 8D 44 24 40 BA 06 00 00 00 E8 ?? ?? ?? ??";
const FADE_CALL_OFFSET: usize = 22;
/// The function behind the event script's `SaveRequest` command.
const SAVE_REQUEST_PATTERN: &str =
    "48 83 EC 28 E8 ?? ?? ?? ?? 84 C0 74 ?? 48 8D 4C 24 30 C6 44 24 30 2A E8 ?? ?? ?? ?? 84 C0 74";
/// The equipment menu's "equip this" routine: `(slot, buffer)`. It reads the inventory
/// index at buffer +0x38 and does the same refreshes as choosing a weapon in the menu.
const EQUIP_ITEM_PATTERN: &str =
    "48 8B C4 57 41 56 41 57 48 81 EC 90 00 00 00 48 C7 40 98 FE FF FF FF 48 89 58 08 48 89 68 10 48 89 70 20 48 8B DA 8B F9 E8";
/// Looks up an item's inventory index: `(inventory, &item_id)`, -1 if it is missing.
const FIND_ITEM_PATTERN: &str =
    "40 56 48 83 EC 20 83 CE FF 83 3A FF 74 59 48 89 5C 24 30 48 89 7C 24 38 48 8D 79 10 48 8B CF E8";
/// Removes one exact inventory entry: `(EquipGameData, index, 1, 1)`. It unequips the
/// item first and is what the event script's remove-item command uses.
const REMOVE_ITEM_PATTERN: &str =
    "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 20 41 0F B6 F1 41 0F B6 E8 8B FA 48 8B D9 E8";
/// Debug creator entry used by the 1.15.2 character spawner. edx 1 creates and edx 2 deletes.
const CREATE_NPC_PATTERN: &str =
    "48 83 EC ?? FF CA 74 ?? FF CA 74 ?? FF CA 75 ?? 48 8B 0D ?? ?? ?? ?? 48 85 C9";
/// WorldChrMan field that points at the debug character creator.
const DEBUG_CHR_CREATOR: usize = 0x3018;
const CREATOR_THINK_ID: usize = 0x124;
const CREATOR_NPC_PARAM_ID: usize = 0x128;
const CREATOR_CHR_ID: usize = 0x1B0;
const CREATOR_DISTANCE: usize = 0x1B4;
const CREATOR_HEIGHT: usize = 0x1B8;
const CREATOR_ANGLE: usize = 0x1BC;
const CREATE_NPC_COMMAND: u32 = 1;
const SPAWN_DISTANCE_METERS: f32 = 3.0;
const SPAWN_DISTANCE_STEPS: u32 = 4;
const SPAWN_HEIGHT_METERS: f32 = 0.5;
/// Set while the creator is idle.
const CREATOR_READY: usize = 0x1C0;
/// The character made by the last request. Creation finishes a few frames later.
const CREATOR_RESULT: usize = 0x1C8;
/// Normal spawns appear within a second, so a longer wait only delays the replacement.
const SPAWN_WAIT_FRAMES: u32 = 120;
/// The game turns an NPC id into a model by dividing by 100, so 110000 is c1100.
const NPC_IDS_PER_MODEL: u32 = 100;
/// Without bosses, rows above this count as boss-like.
const MAX_RANDOM_ENEMY_HP: i32 = 3000;
/// How many failed spawns a batch replaces with another pick.
const MAX_SPAWN_REPLACEMENTS: u32 = 15;


struct Located {
    game_man_slot: Result<usize, String>,
    add_soul: Result<usize, String>,
    set_text_effect: Result<usize, String>,
    fade: Result<FadeSystem, String>,
    scene_slot: Result<usize, String>,
    save_request: Result<usize, String>,
    equip_item: Result<usize, String>,
    find_item: Result<usize, String>,
    remove_item: Result<usize, String>,
    create_npc: Result<usize, String>,
}

#[derive(Clone, Copy)]
struct FadeSystem {
    singleton_slot: usize,
    set_fade: usize,
}

static LOCATED: OnceLock<Located> = OnceLock::new();

/// Scans the executable once. Call this from the worker thread before the frame hook.
pub fn prepare() {
    let _ = LOCATED.get_or_init(|| {
        let located = Located {
            game_man_slot: locate_game_man_slot(),
            add_soul: find_unique(ADD_SOUL_PATTERN),
            set_text_effect: find_unique(SET_TEXT_EFFECT_PATTERN),
            fade: locate_fade_system(),
            scene_slot: locate_scene_slot(),
            save_request: find_unique(SAVE_REQUEST_PATTERN),
            equip_item: find_unique(EQUIP_ITEM_PATTERN),
            find_item: find_unique(FIND_ITEM_PATTERN),
            remove_item: find_unique(REMOVE_ITEM_PATTERN),
            create_npc: find_unique(CREATE_NPC_PATTERN),
        };
        crate::log::log(format!(
            "patterns resolved: session={} souls={} text_effect={} fade={} save={} equip={} spawn={}",
            located.game_man_slot.is_ok(),
            located.add_soul.is_ok(),
            located.set_text_effect.is_ok(),
            located.fade.is_ok(),
            located.save_request.is_ok(),
            located.equip_item.is_ok() && located.find_item.is_ok() && located.remove_item.is_ok(),
            located.create_npc.is_ok(),
        ));
        located
    });
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub exe_match: bool,
    pub session_known: bool,
    pub offline: bool,
    pub in_game: bool,
    pub character_name: String,
}

pub fn snapshot() -> Snapshot {
    let exe_match = current_exe_is_ds3() && patterns_resolve();
    if !exe_match {
        return Snapshot {
            exe_match: false,
            session_known: false,
            offline: false,
            in_game: false,
            character_name: String::new(),
        };
    }
    let (session_known, offline) = match catch_unwind(AssertUnwindSafe(read_offline)) {
        Ok(value) => value,
        Err(_) => (false, false),
    };
    let (in_game, character_name) = match catch_unwind(AssertUnwindSafe(read_character)) {
        Ok(value) => value,
        Err(_) => (false, String::new()),
    };
    Snapshot {
        exe_match,
        session_known,
        offline,
        in_game,
        character_name,
    }
}

pub fn gate() -> Result<(), String> {
    let snapshot = snapshot();
    if !snapshot.exe_match {
        return Err("this Dark Souls III build is not supported".to_string());
    }
    if !snapshot.session_known {
        return Err("the session type is unknown, so effects stay off".to_string());
    }
    if !snapshot.offline {
        return Err("effects run only while the game is in an offline session".to_string());
    }
    if !snapshot.in_game {
        return Err("there is no local player yet".to_string());
    }
    Ok(())
}

/// Why an effect should wait, if the session is fine but the moment is not. Problems
/// that waiting cannot fix, such as being online, are left to [gate].
pub fn not_ready_reason() -> Option<String> {
    let snapshot = snapshot();
    if !snapshot.exe_match || !snapshot.session_known || !snapshot.offline {
        return None;
    }
    let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
    let alive = local_player()
        .is_ok_and(|player| unsafe { &*player }.super_chr_ins.modules.data.hp > 0);
    wait_reason(load_screen_open(), snapshot.in_game, alive, fake_death).map(str::to_string)
}

pub fn wait_reason(
    load_screen: bool,
    in_game: bool,
    alive: bool,
    fake_death: bool,
) -> Option<&'static str> {
    if load_screen {
        Some("a load screen is open")
    } else if !in_game {
        Some("no character is loaded")
    } else if fake_death {
        Some("a fake death is running")
    } else if !alive {
        Some("the player is dead")
    } else {
        None
    }
}

pub fn apply_soul_delta(current: i32, delta: i32) -> i32 {
    let target = (current as i64 + delta as i64).clamp(0, i32::MAX as i64);
    (target - current as i64) as i32
}

/// The souls the game charges to go from this level to the next one.
pub fn next_level_cost(level: i32) -> f64 {
    let level = f64::from(level.max(1));
    let steepness = ((level - 11.0) * 0.02).max(0.0) + 0.1;
    steepness * (level + 81.0).powi(2) + 1.0
}

/// The amount applies as written at the base level and follows the level-up cost from there.
pub fn soul_delta_for_level(delta: i32, level: i32) -> i32 {
    let scale = next_level_cost(level) / next_level_cost(SOUL_SCALE_BASE_LEVEL);
    (f64::from(delta) * scale).round() as i32
}

pub fn adjust_souls(delta: i32, scale_with_level: bool) -> Result<String, String> {
    let pgd = player_game_data()?;
    let delta = if scale_with_level {
        soul_delta_for_level(delta, read_i32(pgd, SOUL_LEVEL_OFFSET)?)
    } else {
        delta
    };
    let before = read_souls(pgd)?;
    let applied = apply_soul_delta(before, delta);
    let target = before.saturating_add(applied);
    call_add_soul(pgd, applied)?;
    let mut after = read_souls(pgd)?;
    if after != target {
        write_souls(pgd, target)?;
        after = read_souls(pgd)?;
    }
    if after != target {
        return Err(format!("souls stayed at {after} after requesting {target}"));
    }
    Ok(format!("souls {before} -> {after}"))
}

pub fn kill_player() -> Result<String, String> {
    if FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some() {
        return Err("a fake death is already running".to_string());
    }
    let player = unsafe { &mut *local_player()? };
    let data = &mut player.super_chr_ins.modules.data;
    let hp = data.hp;
    if hp <= 0 {
        return Err("the local player is already dead".to_string());
    }
    data.hp = 0;
    let after = data.hp;
    if after != 0 {
        return Err(format!("player HP stayed at {after}"));
    }
    Ok(format!("player HP {hp} -> 0; the game will handle the death normally"))
}

const FAKE_DEATH_HOLD_FRAMES: u32 = 90;
const FAKE_DEATH_RETRY_FRAMES: u32 = 90;
const TRUE_DEATH_OFFSET: usize = 0x90;
const TRUE_DEATH_NUM_OFFSET: usize = 0x94;
const DEATH_NUM_OFFSET: usize = 0x98;
const MODULES_OFFSET: usize = 0x1F90;
const DATA_MODULE_OFFSET: usize = 0x18;
const EVENT_MODULE_OFFSET: usize = 0x58;
const ACTION_REQUEST_OFFSET: usize = 0x80;
const IDLE_ANIM_OFFSET: usize = 0x20;
const CURRENT_ANIM_OFFSET: usize = 0xC8;
const DATA_FLAGS_OFFSET: usize = 0x1C0;
const CHR_STATE_OFFSET: usize = 0x1EE8;
const FALL_DEATH_CAM_OFFSET: usize = 0x90;
/// Load-in-area animation used by the proven Auto Revive trainer script.
const REVIVE_ANIMATION: i32 = 63000;
/// Text effect type of the F23_YouDied banner. The death handler uses the same value.
const TEXT_EFFECT_YOU_DIED: u32 = 2;
/// A real death runs off the dead bit, whichever animation played: the banner and
/// the grey start with it, and the fade to black follows this long after.
/// Measured on a real death (3745 ms to 7967 ms).
const STAGED_BLACK_AFTER_DEAD: Duration = Duration::from_millis(4220);
const STAGED_BLACK_FADE: Duration = Duration::from_millis(500);
/// When a real death starts the banner and grey, measured once.
const STAGED_DEAD_AFTER: Duration = Duration::from_millis(3745);
const STAGED_BLACK_HOLD_FRAMES: u32 = 30;
/// Fade plate that holds the death grey. The game starts it when it sees the dead
/// bit and only clears it on the respawn path, so it stays until something resets it.
const GREY_FADE_PLATE: u32 = 6;
/// Fade plate the respawn path uses for its fade to black.
const BLACK_FADE_PLATE: u32 = 2;
/// The game's own length for both directions of the fade to black.
const BLACK_FADE_SECONDS: f32 = 0.5;
const FADE_CLEAR: [f32; 4] = [0.0; 4];
const FADE_BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
/// What the game itself fades plate 6 to when it sees the dead bit.
const FADE_GREY: [f32; 4] = [1.0, 1.0, 1.0, 0.0];
const GREY_FADE_SECONDS: f32 = 1.5;
/// The death event's only test is bit 0 of the byte at
/// `chr_set_entry -> +0x30 -> +0x8C`. Setting it starts the respawn chain.
const CHR_SET_ENTRY_OFFSET: usize = 0x18;
const DEAD_STATE_OFFSET: usize = 0x30;
const DEAD_FLAGS_OFFSET: usize = 0x8C;
const DEAD_EVENT_BIT: u8 = 1;

struct FakeDeath {
    saved_hp: i32,
    saved_souls: i32,
    saved_true_death: i32,
    saved_true_death_num: i32,
    saved_death_num: i32,
    saved_stain_souls: i32,
    saved_flags: u32,
    saved_chr_state: u32,
    remaining: u32,
    restoring: bool,
    restore_failures: u32,
    started: Instant,
    staged: StagedDeath,
}

/// Progress of the fake death that shows YOU DIED before standing up.
#[derive(Default)]
struct StagedDeath {
    banner_queued: bool,
    black_on: bool,
}

static FAKE_DEATH: Mutex<Option<FakeDeath>> = Mutex::new(None);

pub fn start_fake_death() -> Result<String, String> {
    let saved_hp = begin_fake_death()?;
    Ok(format!(
        "Fake death started from {saved_hp} HP. YOU DIED shows during the fall, then you stand back up at {saved_hp}."
    ))
}

fn begin_fake_death() -> Result<i32, String> {
    let player = unsafe { &mut *local_player()? };
    let player_addr = player as *mut PlayerIns as usize;
    let saved_hp = player.super_chr_ins.modules.data.hp;
    if saved_hp <= 0 {
        return Err("the local player is already dead".to_string());
    }
    let saved_souls = read_souls(player.player_game_data.as_ptr())?;
    let (saved_true_death, saved_true_death_num, saved_death_num, saved_stain_souls) =
        death_counters()?;
    let saved_flags = data_flags(player_addr)?;
    let saved_chr_state = chr_state(player_addr)?;
    let mut slot = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner());
    if slot.is_some() {
        return Err("a fake death is already running".to_string());
    }
    player.super_chr_ins.modules.data.hp = 0;
    *slot = Some(FakeDeath {
        saved_hp,
        saved_souls,
        saved_true_death,
        saved_true_death_num,
        saved_death_num,
        saved_stain_souls,
        saved_flags,
        saved_chr_state,
        remaining: 0,
        restoring: false,
        restore_failures: 0,
        started: Instant::now(),
        staged: StagedDeath::default(),
    });
    Ok(saved_hp)
}

/// Queues the banner the same way the death event does. The text-effect dialog
/// plays the letters and the sting on its own update. The bonfire reload is only
/// armed by the death handler, so nothing else follows while the player is alive.
pub fn show_you_died() -> Result<String, String> {
    let address = *cached()?.set_text_effect.as_ref().map_err(|error| error.clone())?;
    let set_text_effect: extern "C" fn(u32, u8) -> u8 = unsafe { std::mem::transmute(address) };
    if set_text_effect(TEXT_EFFECT_YOU_DIED, 0) == 0 {
        return Err("the game rejected the YOU DIED text effect".to_string());
    }
    let message = "queued the YOU DIED banner".to_string();
    Ok(message)
}

pub fn poll_fake_death() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(fake) = slot.as_mut() else {
            return;
        };
        if load_screen_open() {
            let _ = persist_fake_death_life(fake);
            release_black(fake);
            crate::log::log("fake death stopped because a load screen is open; load flags were not touched");
            slot.take();
            return;
        }
        if !fake.restoring {
            let _ = write_souls_if_player(fake.saved_souls);
            let _ = write_persistent_hp(fake.saved_hp);
            if !advance_staged_death(fake) {
                return;
            }
            fake.restoring = true;
            fake.remaining = FAKE_DEATH_HOLD_FRAMES;
            if let Err(error) = set_fade_plate(GREY_FADE_PLATE, &FADE_CLEAR, 0.0) {
                crate::log::log(format!("staged fake death grey reset failed: {error}"));
            }
        }
        match restore_fake_death(fake) {
            Ok(()) => {
                fake.restore_failures = 0;
                fake.remaining = fake.remaining.saturating_sub(1);
                if fake.remaining <= FAKE_DEATH_HOLD_FRAMES - STAGED_BLACK_HOLD_FRAMES {
                    release_black(fake);
                }
                if fake.remaining == 0 {
                    save_living_player();
                    slot.take();
                }
            }
            Err(error) => {
                let _ = persist_fake_death_life(fake);
                fake.restore_failures = fake.restore_failures.saturating_add(1);
                if fake.restore_failures == 1 || fake.restore_failures % 60 == 0 {
                    crate::log::log(format!("fake death restore failed: {error}"));
                }
                if fake.restore_failures >= FAKE_DEATH_RETRY_FRAMES {
                    crate::log::log(
                        "fake death gave up restoring the character; death counters and saved HP were written back",
                    );
                    release_black(fake);
                    save_living_player();
                    slot.take();
                }
            }
        }
    }));
}

/// Runs one frame of the staged death. Returns true once it is time to stand up.
fn advance_staged_death(fake: &mut FakeDeath) -> bool {
    let since_start = fake.started.elapsed();
    let staged = &mut fake.staged;
    let Ok(player) = local_player() else {
        return false;
    };
    let player = unsafe { &mut *player };
    let player_addr = player as *mut PlayerIns as usize;

    player.super_chr_ins.modules.data.hp = 0;
    if let Err(error) = clear_dead_event_bit(player_addr) {
        crate::log::log(format!("staged fake death dead bit: {error}"));
    }

    if since_start < STAGED_DEAD_AFTER {
        return false;
    }
    if !staged.banner_queued {
        staged.banner_queued = true;
        if let Err(error) = show_you_died() {
            crate::log::log(format!("staged fake death banner failed: {error}"));
        }
        if let Err(error) = set_fade_plate(GREY_FADE_PLATE, &FADE_GREY, GREY_FADE_SECONDS) {
            crate::log::log(format!("staged fake death grey failed: {error}"));
        }
    }
    let since_dead = since_start - STAGED_DEAD_AFTER;
    if since_dead >= STAGED_BLACK_AFTER_DEAD && !staged.black_on {
        staged.black_on = true;
        if let Err(error) = set_fade_plate(BLACK_FADE_PLATE, &FADE_BLACK, BLACK_FADE_SECONDS) {
            crate::log::log(format!("staged fake death fade to black failed: {error}"));
        }
    }
    since_dead >= STAGED_BLACK_AFTER_DEAD + STAGED_BLACK_FADE
}

/// Every way out of a staged death must come through here, or the screen stays black.
fn release_black(fake: &mut FakeDeath) {
    if !fake.staged.black_on {
        return;
    }
    fake.staged.black_on = false;
    if let Err(error) = set_fade_plate(BLACK_FADE_PLATE, &FADE_CLEAR, BLACK_FADE_SECONDS) {
        crate::log::log(format!("staged fake death fade from black failed: {error}"));
    }
}

fn dead_flags_address(player: usize) -> Result<usize, String> {
    let entry = read_usize_checked(player.wrapping_add(CHR_SET_ENTRY_OFFSET))?;
    if entry < 0x10_000 {
        return Err("the player's chr set entry is missing".to_string());
    }
    let state = read_usize_checked(entry.wrapping_add(DEAD_STATE_OFFSET))?;
    if state < 0x10_000 {
        return Err("the player's dead state is missing".to_string());
    }
    Ok(state.wrapping_add(DEAD_FLAGS_OFFSET))
}

/// Returns true if the bit was set.
fn clear_dead_event_bit(player: usize) -> Result<bool, String> {
    let address = dead_flags_address(player)?;
    let flags = read_u32_checked(address)? as u8;
    if flags & DEAD_EVENT_BIT == 0 {
        return Ok(false);
    }
    write_bytes_checked(address, &[flags & !DEAD_EVENT_BIT])?;
    Ok(true)
}

/// The game saves the moment HP reaches 0, so without this the file on disk keeps
/// the dying character until the next autosave, and a crash brings that back.
fn save_living_player() {
    let address = match cached().and_then(|located| {
        located.save_request.as_ref().map(|address| *address).map_err(|error| error.clone())
    }) {
        Ok(address) => address,
        Err(error) => {
            crate::log::log(format!("fake death save request failed: {error}"));
            return;
        }
    };
    let request_save: extern "C" fn() = unsafe { std::mem::transmute(address) };
    request_save();
}

fn locate_scene_slot() -> Result<usize, String> {
    let site = find_unique(SCENE_PATTERN)?;
    Ok(pattern::rip_target(site, 3, 7)?)
}

fn locate_fade_system() -> Result<FadeSystem, String> {
    let site = find_unique(FADE_PATTERN)?;
    Ok(FadeSystem {
        singleton_slot: pattern::rip_target(site, 3, 7)?,
        set_fade: pattern::rip_target(site + FADE_CALL_OFFSET, 1, 5)?,
    })
}

/// The game reads the duration from offset 8 of its time object.
#[repr(C)]
struct FadeDuration {
    _vftable: usize,
    seconds: f32,
}

fn fade_singleton() -> Result<(FadeSystem, usize), String> {
    let system = *cached()?.fade.as_ref().map_err(|error| error.clone())?;
    let fade = read_usize_checked(system.singleton_slot)?;
    if fade < 0x10_000 {
        return Err("the fade system is missing".to_string());
    }
    Ok((system, fade))
}

/// Moves a fade plate to a colour. Zero seconds applies it at once.
fn set_fade_plate(plate: u32, color: &[f32; 4], seconds: f32) -> Result<(), String> {
    let (system, fade) = fade_singleton()?;
    let set_fade: extern "C" fn(usize, u32, *const [f32; 4], *const FadeDuration) =
        unsafe { std::mem::transmute(system.set_fade) };
    let duration = FadeDuration {
        _vftable: 0,
        seconds,
    };
    set_fade(fade, plate, color, &duration);
    Ok(())
}

const CHR_CTRL_OFFSET: usize = 0x50;
/// Hit flags of the character controller. The game keeps a copy at +0x188 and only
/// recalculates hit layers when the two differ, so the copy must be left alone here.
const HIT_FLAGS_OFFSET: usize = 0x186;
/// The game's death routine sets this as the death animation ends. While it is set, the
/// game puts the player on corpse collision layers and attacks pass straight through.
const HITS_OFF_BIT: u16 = 1;

fn read_u16_checked(address: usize) -> Result<u16, String> {
    let mut bytes = [0u8; 2];
    if copy_readable(address, &mut bytes) != 2 {
        return Err(format!("address {address:#X} is not readable"));
    }
    Ok(u16::from_ne_bytes(bytes))
}

fn enable_player_hits(player: usize) -> Result<(), String> {
    let flags_address = read_usize_checked(player.wrapping_add(CHR_CTRL_OFFSET))?.wrapping_add(HIT_FLAGS_OFFSET);
    let flags = read_u16_checked(flags_address)?;
    if flags & HITS_OFF_BIT != 0 {
        write_bytes_checked(flags_address, &(flags & !HITS_OFF_BIT).to_ne_bytes())?;
    }
    Ok(())
}

fn restore_fake_death(fake: &FakeDeath) -> Result<(), String> {
    let player = unsafe { &mut *local_player()? };
    let _ = clear_dead_event_bit(player as *mut PlayerIns as usize);
    write_player_hp(player, fake.saved_hp);
    write_souls(player.player_game_data.as_ptr(), fake.saved_souls)?;
    persist_fake_death_life(fake)?;
    let player_addr = player as *mut PlayerIns as usize;
    write_data_flags(player_addr, fake.saved_flags)?;
    write_chr_state(player_addr, fake.saved_chr_state)?;
    enable_player_hits(player_addr)?;
    write_revive_animation(player_addr)?;
    clear_fall_death_camera()?;
    Ok(())
}

fn persist_fake_death_life(fake: &FakeDeath) -> Result<(), String> {
    write_death_counters(
        fake.saved_true_death,
        fake.saved_true_death_num,
        fake.saved_death_num,
        fake.saved_stain_souls,
    )?;
    write_persistent_hp(fake.saved_hp)
}

fn write_souls_if_player(souls: i32) -> Result<(), String> {
    let player = unsafe { &mut *local_player()? };
    write_souls(player.player_game_data.as_ptr(), souls)
}

fn write_player_hp(player: &mut PlayerIns, hp: i32) {
    player.super_chr_ins.modules.data.hp = hp;
    unsafe {
        player.player_game_data.as_mut().player_info.hp = hp.max(0) as u32;
    }
}

fn write_persistent_hp(hp: i32) -> Result<(), String> {
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    game_data.main_player_game_data.player_info.hp = hp.max(0) as u32;
    Ok(())
}

fn module_pointer(player: usize, offset: usize) -> Result<usize, String> {
    let modules = read_usize_checked(player.wrapping_add(MODULES_OFFSET))?;
    if modules < 0x10_000 {
        return Err("player modules are missing".to_string());
    }
    let pointer = read_usize_checked(modules.wrapping_add(offset))?;
    if pointer < 0x10_000 {
        return Err("player module pointer is missing".to_string());
    }
    Ok(pointer)
}

fn data_flags(player: usize) -> Result<u32, String> {
    let data = module_pointer(player, DATA_MODULE_OFFSET)?;
    read_u32_checked(data.wrapping_add(DATA_FLAGS_OFFSET))
}

fn write_data_flags(player: usize, flags: u32) -> Result<(), String> {
    let data = module_pointer(player, DATA_MODULE_OFFSET)?;
    write_u32_checked(data.wrapping_add(DATA_FLAGS_OFFSET), flags)
}

fn chr_state(player: usize) -> Result<u32, String> {
    read_u32_checked(player.wrapping_add(CHR_STATE_OFFSET))
}

fn write_chr_state(player: usize, state: u32) -> Result<(), String> {
    write_u32_checked(player.wrapping_add(CHR_STATE_OFFSET), state)
}

fn write_revive_animation(player: usize) -> Result<(), String> {
    let event = module_pointer(player, EVENT_MODULE_OFFSET)?;
    write_i32_checked(event.wrapping_add(IDLE_ANIM_OFFSET), REVIVE_ANIMATION)?;
    if let Ok(action) = module_pointer(player, ACTION_REQUEST_OFFSET) {
        let _ = write_i32_checked(action.wrapping_add(CURRENT_ANIM_OFFSET), REVIVE_ANIMATION);
    }
    Ok(())
}

fn clear_fall_death_camera() -> Result<(), String> {
    let world = unsafe { WorldChrMan::instance_mut() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?;
    let base = world as *mut WorldChrMan as *mut u8;
    unsafe { base.add(FALL_DEATH_CAM_OFFSET).write(0) };
    Ok(())
}

fn read_u32_checked(addr: usize) -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    if copy_readable(addr, &mut bytes) != 4 {
        return Err(format!("address {addr:#X} is not readable"));
    }
    Ok(u32::from_ne_bytes(bytes))
}

fn write_u32_checked(addr: usize, value: u32) -> Result<(), String> {
    write_bytes_checked(addr, &value.to_ne_bytes())
}

fn write_i32_checked(addr: usize, value: i32) -> Result<(), String> {
    write_bytes_checked(addr, &value.to_ne_bytes())
}

fn write_bytes_checked(addr: usize, bytes: &[u8]) -> Result<(), String> {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if addr < 0x10_000 {
        return Err(format!("address {addr:#X} is not writable"));
    }
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let written = unsafe {
        VirtualQuery(
            Some(addr as *const _),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if written == 0 || info.State != MEM_COMMIT {
        return Err(format!("address {addr:#X} is not writable"));
    }
    let protect = info.Protect.0 & 0xFF;
    let writable = matches!(protect, 0x04 | 0x08 | 0x40 | 0x80);
    let guarded = (info.Protect.0 & 0x100) != 0;
    let region_end = (info.BaseAddress as usize).saturating_add(info.RegionSize);
    if !writable || guarded || addr + bytes.len() > region_end {
        return Err(format!("address {addr:#X} is not writable"));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
    }
    Ok(())
}

fn death_counters() -> Result<(i32, i32, i32, i32), String> {
    let game_data = unsafe { GameDataMan::instance() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    let base = game_data as *const GameDataMan as *const u8;
    let true_death = unsafe { base.add(TRUE_DEATH_OFFSET).cast::<i32>().read() };
    let true_death_num = unsafe { base.add(TRUE_DEATH_NUM_OFFSET).cast::<i32>().read() };
    let death_num = unsafe { base.add(DEATH_NUM_OFFSET).cast::<i32>().read() };
    let stain = game_data.bloodstain.as_ref().souls;
    Ok((true_death, true_death_num, death_num, stain))
}

fn write_death_counters(
    true_death: i32,
    true_death_num: i32,
    death_num: i32,
    stain_souls: i32,
) -> Result<(), String> {
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    let base = game_data as *mut GameDataMan as *mut u8;
    unsafe {
        base.add(TRUE_DEATH_OFFSET).cast::<i32>().write(true_death);
        base.add(TRUE_DEATH_NUM_OFFSET)
            .cast::<i32>()
            .write(true_death_num);
        base.add(DEATH_NUM_OFFSET).cast::<i32>().write(death_num);
        game_data.bloodstain.as_mut().souls = stain_souls;
    }
    Ok(())
}

fn load_screen_open() -> bool {
    unsafe { MenuMan::instance() }
        .map(|menu| menu.is_load_screen())
        .unwrap_or(false)
}

pub fn health_at_percent(max_hp: i32, percent: i32) -> i32 {
    if max_hp <= 0 || percent <= 0 {
        return 0;
    }
    ((max_hp as i64 * percent.clamp(0, 100) as i64) / 100)
        .clamp(1, max_hp as i64) as i32
}

pub fn empty_stamina() -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    let data = &mut player.super_chr_ins.modules.data;
    if data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let before = data.stamina;
    data.stamina = 0;
    Ok(format!("player stamina {before} -> 0"))
}

/// The death grey's own path to the scene object, which holds the colour correction.
const SCENE_PATTERN: &str = "48 8B 05 ?? ?? ?? ?? 48 8B 58 10 BA 06 00 00 00 48 8B CF E8";
const SCENE_OWNER_SCENE: usize = 0x10;
const SCENE_DRAW_SOURCE: usize = 0x340;
const DRAW_SOURCE_GET_PARAMS: usize = 0x18;
/// How strongly the colour correction applies, from 0 to 1.
const COLOR_BLEND_RATE: usize = 0x3928;
/// Saturation for red, green and blue. The death grey sets these to 0.
const COLOR_SATURATION: usize = 0x3930;

struct Oversaturation {
    until: Instant,
    saturation: f32,
    logged: bool,
}

static OVERSATURATION: Mutex<Option<Oversaturation>> = Mutex::new(None);

pub fn oversaturate(seconds: i32, percent: i32) -> Result<String, String> {
    cached()?.scene_slot.as_ref().map_err(|error| error.clone())?;
    let until = Instant::now() + Duration::from_secs(seconds.max(1) as u64);
    let saturation = percent as f32 / 100.0;
    let mut slot = OVERSATURATION.lock().unwrap_or_else(|poison| poison.into_inner());
    match slot.as_mut() {
        Some(state) => {
            state.until = state.until.max(until);
            state.saturation = saturation;
        }
        None => {
            *slot = Some(Oversaturation {
                until,
                saturation,
                logged: false,
            });
        }
    }
    Ok(format!("colours at {percent}% for {seconds} seconds"))
}

fn color_params() -> Result<usize, String> {
    let slot = *cached()?.scene_slot.as_ref().map_err(|error| error.clone())?;
    let owner = read_usize_checked(slot)?;
    let scene = read_usize_checked(owner + SCENE_OWNER_SCENE)?;
    let source = read_usize_checked(scene + SCENE_DRAW_SOURCE)?;
    let getter = read_usize_checked(read_usize_checked(source)? + DRAW_SOURCE_GET_PARAMS)?;
    let call: extern "C" fn(usize) -> usize = unsafe { std::mem::transmute(getter) };
    let params = call(source);
    if params < 0x10_000 {
        return Err("the colour settings are missing".to_string());
    }
    Ok(params)
}

/// The game rewrites these every frame, so this runs after that and before the scene draws.
pub fn poll_oversaturation() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = OVERSATURATION.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(state) = slot.as_mut() else {
            return;
        };
        if Instant::now() >= state.until {
            slot.take();
            return;
        }
        let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        if fake_death || load_screen_open() {
            return;
        }
        let params = match color_params() {
            Ok(params) => params,
            Err(error) => {
                if !state.logged {
                    state.logged = true;
                    crate::log::log(format!("oversaturate: {error}"));
                }
                return;
            }
        };
        if !state.logged {
            state.logged = true;
        }
        let saturation = state.saturation.to_ne_bytes();
        let _ = write_bytes_checked(params + COLOR_BLEND_RATE, &1f32.to_ne_bytes());
        for channel in 0..3 {
            let _ = write_bytes_checked(params + COLOR_SATURATION + channel * 4, &saturation);
        }
    }));
}

/// When the screen starts fading back in.
static BLIND_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

/// A second use while blind only extends the hold.
pub fn blind(seconds: i32) -> Result<String, String> {
    if FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some() {
        return Err("Fake Death is using the black screen".to_string());
    }
    let hold = Duration::from_secs(seconds.max(1) as u64);
    let mut slot = BLIND_UNTIL.lock().unwrap_or_else(|poison| poison.into_inner());
    let until = match *slot {
        Some(current) => current.max(Instant::now() + hold),
        None => {
            set_fade_plate(BLACK_FADE_PLATE, &FADE_BLACK, BLACK_FADE_SECONDS)?;
            Instant::now() + Duration::from_secs_f32(BLACK_FADE_SECONDS) + hold
        }
    };
    *slot = Some(until);
    Ok(format!("blind for {seconds} seconds"))
}

/// A load screen waits, so the fade back in cannot be lost and leave the screen black.
pub fn poll_blind() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = BLIND_UNTIL.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(until) = *slot else {
            return;
        };
        if Instant::now() < until || load_screen_open() {
            return;
        }
        if let Err(error) = set_fade_plate(BLACK_FADE_PLATE, &FADE_CLEAR, BLACK_FADE_SECONDS) {
            crate::log::log(format!("blind fade back in is waiting: {error}"));
            return;
        }
        slot.take();
    }));
}

static STAMINA_LOCK: Mutex<Option<Instant>> = Mutex::new(None);
/// Rolling and attacking still worked with stamina held at exactly 0. Below 0 is the
/// overdrawn state the game itself refuses actions in, with room for a frame of regen.
const NO_STAMINA_VALUE: i32 = -1000;

pub fn no_stamina(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    let data = &mut player.super_chr_ins.modules.data;
    if data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    data.stamina = NO_STAMINA_VALUE;
    let until = Instant::now() + Duration::from_secs(seconds.max(0) as u64);
    let mut slot = STAMINA_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    *slot = Some(slot.map_or(until, |current| current.max(until)));
    Ok(format!("stamina held empty for {seconds} seconds"))
}

/// Stamina regenerates during the character update and any amount above 0 allows an
/// action, so this runs at frame start and again just before the player's behaviour.
pub fn poll_stamina_lock() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = STAMINA_LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(until) = *slot else {
            return;
        };
        let finished = Instant::now() >= until || load_screen_open();
        if let Ok(player) = local_player() {
            let value = if finished { 0 } else { NO_STAMINA_VALUE };
            unsafe { &mut *player }.super_chr_ins.modules.data.stamina = value;
        }
        if finished {
            slot.take();
        }
    }));
}

const BEHAVIOR_MODULE_OFFSET: usize = 0x28;
/// Playback multiplier: the module's update multiplies frame time by this float.
const PLAYBACK_SPEED_OFFSET: usize = 0xA58;
const NORMAL_SPEED: f32 = 1.0;
const MIN_SPEED: f32 = 0.1;
const MAX_SPEED: f32 = 4.0;

struct SpeedChange {
    percent: i32,
    until: Instant,
    movement_only: bool,
}

static SPEED_CHANGES: Mutex<Vec<SpeedChange>> = Mutex::new(Vec::new());
/// The animation id drops back to movement during an attack's recovery, so wait this
/// long after any action before slowing down again.
const SPEED_ACTION_GRACE: Duration = Duration::from_millis(250);
static SPEED_LAST_ACTION: Mutex<Option<Instant>> = Mutex::new(None);

/// Active changes add together, so -30% and +30% give normal speed.
pub fn combined_speed(percents: impl Iterator<Item = i32>) -> f32 {
    let total: i32 = percents.sum();
    (NORMAL_SPEED + total as f32 / 100.0).clamp(MIN_SPEED, MAX_SPEED)
}

/// Measured in-game: 0 is idle, 20010-22100 are walking, running, turning and stopping,
/// and 27115 is a roll. Attacks (230xxxxx) and items (150032100) sit outside this.
pub fn is_movement_animation(anim: i32) -> bool {
    anim <= 0 || (20_000..23_000).contains(&anim) || (27_000..28_000).contains(&anim)
}

fn write_player_speed(player: usize, speed: f32) -> Result<(), String> {
    let module = module_pointer(player, BEHAVIOR_MODULE_OFFSET)?;
    write_u32_checked(module.wrapping_add(PLAYBACK_SPEED_OFFSET), speed.to_bits())
}

pub fn change_speed(percent: i32, seconds: i32, movement_only: bool) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let player = player as *mut PlayerIns as usize;
    let mut changes = SPEED_CHANGES.lock().unwrap_or_else(|poison| poison.into_inner());
    changes.push(SpeedChange {
        percent,
        until: Instant::now() + Duration::from_secs(seconds.max(0) as u64),
        movement_only,
    });
    let speed = combined_speed(changes.iter().map(|change| change.percent));
    write_player_speed(player, speed)?;
    Ok(format!("speed {percent:+}% for {seconds} seconds, now {speed:.2}x"))
}

/// Runs several times a frame, so it reads the live player directly instead of going
/// through the checked helpers, which query the memory map on every access.
fn apply_speed_for_animation(player: usize, moving: f32, acting: f32, now: Instant) {
    let module = |offset: usize| unsafe {
        let modules = ((player + MODULES_OFFSET) as *const usize).read();
        if modules < 0x10_000 {
            return None;
        }
        let pointer = ((modules + offset) as *const usize).read();
        (pointer >= 0x10_000).then_some(pointer)
    };
    let (Some(action), Some(behavior)) = (module(ACTION_REQUEST_OFFSET), module(BEHAVIOR_MODULE_OFFSET))
    else {
        return;
    };
    let anim = unsafe { ((action + CURRENT_ANIM_OFFSET) as *const i32).read() };

    // The multiplier covers every animation, so movement-only changes drop out for actions.
    let mut last_action = SPEED_LAST_ACTION.lock().unwrap_or_else(|poison| poison.into_inner());
    if !is_movement_animation(anim) {
        *last_action = Some(now);
    }
    let recently_acted =
        last_action.is_some_and(|at| now.saturating_duration_since(at) < SPEED_ACTION_GRACE);
    let applied = if recently_acted { acting } else { moving };
    unsafe { ((behavior + PLAYBACK_SPEED_OFFSET) as *mut f32).write(applied) };
}

pub fn poll_speed_changes() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut changes = SPEED_CHANGES.lock().unwrap_or_else(|poison| poison.into_inner());
        if changes.is_empty() {
            return;
        }
        if load_screen_open() {
            changes.clear();
            return;
        }
        let now = Instant::now();
        changes.retain(|change| change.until > now);
        let moving = combined_speed(changes.iter().map(|change| change.percent));
        let acting = combined_speed(
            changes
                .iter()
                .filter(|change| !change.movement_only)
                .map(|change| change.percent),
        );
        if let Ok(player) = local_player() {
            apply_speed_for_animation(player as usize, moving, acting, now);
        }
    }));
}

/// Just above zero, so the animation update never sees a frame time of nothing.
const FROZEN_SPEED: f32 = 0.001;

static FREEZE_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

pub fn freeze(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    if FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some() {
        return Err("a fake death is running".to_string());
    }
    let until = Instant::now() + Duration::from_secs(seconds.max(0) as u64);
    let mut slot = FREEZE_UNTIL.lock().unwrap_or_else(|poison| poison.into_inner());
    *slot = Some(slot.map_or(until, |current| current.max(until)));
    crate::input::block_input(seconds);
    Ok(format!("player frozen for {seconds} seconds"))
}

/// Runs after the speed changes, so a freeze wins over them. They take over again when it ends.
pub fn poll_freeze() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = FREEZE_UNTIL.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(until) = *slot else {
            return;
        };
        let now = Instant::now();
        let player = local_player();
        let alive = player
            .as_ref()
            .is_ok_and(|player| unsafe { &**player }.super_chr_ins.modules.data.hp > 0);
        let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        // A frozen death animation would hold up the respawn.
        let finished = now >= until || !alive || fake_death || load_screen_open();
        if !finished {
            if let Ok(player) = player {
                apply_speed_for_animation(player as usize, FROZEN_SPEED, FROZEN_SPEED, now);
            }
            return;
        }
        slot.take();
        crate::input::stop_blocking_input();
        if let Ok(player) = player {
            let _ = write_player_speed(player as usize, NORMAL_SPEED);
        }
    }));
}

pub fn one_hp() -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    let data = &mut player.super_chr_ins.modules.data;
    if data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let before = data.hp;
    data.hp = 1;
    Ok(format!("player HP {before} -> 1"))
}

static ONE_HIT_RUN: Mutex<Option<Instant>> = Mutex::new(None);

pub fn one_hit_run(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    let data = &mut player.super_chr_ins.modules.data;
    if data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    data.hp = 1;
    let until = Instant::now() + Duration::from_secs(seconds.max(0) as u64);
    let mut slot = ONE_HIT_RUN.lock().unwrap_or_else(|poison| poison.into_inner());
    *slot = Some(slot.map_or(until, |current| current.max(until)));
    Ok(format!("HP held at 1 for {seconds} seconds"))
}

/// Only ever lowers HP, so a killing hit at 1 HP stays a real death.
pub fn poll_one_hit_run() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = ONE_HIT_RUN.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(until) = *slot else {
            return;
        };
        if Instant::now() >= until || load_screen_open() {
            slot.take();
            return;
        }
        if let Ok(player) = local_player() {
            let data = &mut unsafe { &mut *player }.super_chr_ins.modules.data;
            if data.hp > 1 {
                data.hp = 1;
            }
        }
    }));
}

/// Data-module byte used by the debug "No Damage" switch. Bit 1 blocks hit damage.
const NO_DAMAGE_FLAG_OFFSET: usize = 0x1C0;
const NO_DAMAGE_BIT: u8 = 1 << 1;

struct Invincibility {
    until: Instant,
    clear_on_end: bool,
}

static INVINCIBILITY: Mutex<Option<Invincibility>> = Mutex::new(None);

pub fn with_no_damage_bit(flags: u8, enabled: bool) -> u8 {
    if enabled {
        flags | NO_DAMAGE_BIT
    } else {
        flags & !NO_DAMAGE_BIT
    }
}

fn set_no_damage(player: &mut PlayerIns, enabled: bool) -> Result<bool, String> {
    let data = player.super_chr_ins.modules.data.as_ptr() as usize;
    if data < 0x10_000 {
        return Err("player data is missing".to_string());
    }
    let address = data + NO_DAMAGE_FLAG_OFFSET;
    let mut byte = [0u8; 1];
    if copy_readable(address, &mut byte) != 1 {
        return Err("the no-damage flag is not readable".to_string());
    }
    let was_set = byte[0] & NO_DAMAGE_BIT != 0;
    let next = with_no_damage_bit(byte[0], enabled);
    if next != byte[0] {
        write_bytes_checked(address, &[next])?;
    }
    Ok(was_set)
}

pub fn invincible(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let already = set_no_damage(player, true)?;
    let requested = Instant::now() + Duration::from_secs(seconds.max(0) as u64);
    let mut slot = INVINCIBILITY.lock().unwrap_or_else(|poison| poison.into_inner());
    let until = match slot.as_mut() {
        Some(current) => {
            current.until = current.until.max(requested);
            current.until
        }
        None => {
            *slot = Some(Invincibility {
                until: requested,
                clear_on_end: !already,
            });
            requested
        }
    };
    let remaining = until.saturating_duration_since(Instant::now()).as_secs().max(1);
    Ok(format!("invincible for {remaining} seconds"))
}

pub fn poll_invincibility() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = INVINCIBILITY.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(state) = slot.as_mut() else {
            return;
        };
        let finished = Instant::now() >= state.until || load_screen_open();
        let enabled = !finished || !state.clear_on_end;
        if let Ok(player) = local_player() {
            let _ = set_no_damage(unsafe { &mut *player }, enabled);
        }
        if finished {
            slot.take();
        }
    }));
}

const RIGHT_HAND_SLOT: u32 = 1;
const EQUIP_BUFFER_SIZE: usize = 0x100;
const EQUIP_BUFFER_INVENTORY_INDEX: usize = 0x38;
/// -1 here makes the menu routine empty the slot.
const EQUIP_BUFFER_UNEQUIP: usize = 0x3C;
/// Bit 0 here allows equipment changes outside the menu.
const EQUIP_UNLOCK_MODULE_OFFSET: usize = 0;
const EQUIP_UNLOCK_FLAGS_OFFSET: usize = 0x10;

/// Base weapons only: infusions add 100 to the id and upgrades add 1. Ids below 1000000
/// are fists and ids from 20000000 are shields and ammo.
pub fn is_base_weapon_id(id: u64) -> bool {
    id % 10_000 == 0 && (1_000_000..20_000_000).contains(&id)
}

fn random_index(length: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        .wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed));
    let mixed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((mixed >> 32) as usize) % length.max(1)
}

/// How one property of the new weapon is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeaponTrait {
    Base,
    Random,
    SameAsRightHand,
}

impl WeaponTrait {
    pub fn from_index(index: i64) -> Self {
        match index {
            1 => Self::Random,
            2 => Self::SameAsRightHand,
            _ => Self::Base,
        }
    }
}

pub struct WeaponOptions {
    pub level: WeaponTrait,
    pub infusion: WeaponTrait,
    pub usable_only: bool,
    pub remove_old: bool,
    pub include_missing_dlc: bool,
}

/// The weapon data holds DLC weapons whether or not the DLC is installed, and its
/// `simple_model_for_dlc` flag is unset for every weapon, so these are listed by hand.
/// All ids were checked against the game's data.
const ASHES_OF_ARIANDEL_WEAPONS: [u32; 13] = [
    2_240_000, 4_120_000, 7_170_000, 7_180_000, 8_310_000, 9_230_000, 10_180_000, 11_050_000,
    12_070_000, 13_410_000, 14_180_000, 16_140_000, 16_150_000,
];
const RINGED_CITY_WEAPONS: [u32; 19] = [
    1_140_000, 1_150_000, 2_250_000, 2_260_000, 4_130_000, 5_060_000, 6_380_000, 8_320_000,
    9_240_000, 9_250_000, 9_260_000, 10_170_000, 13_420_000, 13_430_000, 13_440_000, 14_120_000,
    14_190_000, 16_120_000, 16_160_000,
];

pub fn weapon_available(base: u32, ashes_installed: bool, ringed_city_installed: bool) -> bool {
    (ashes_installed || !ASHES_OF_ARIANDEL_WEAPONS.contains(&base))
        && (ringed_city_installed || !RINGED_CITY_WEAPONS.contains(&base))
}

const INFUSION_COUNT: u32 = 16;
const MAX_WEAPON_LEVEL: u32 = 15;

/// Fists fill an empty hand and sit below the real weapon ids.
pub fn is_removable_weapon(id: u32) -> bool {
    id >= 1_000_000
}

/// A weapon id is the base id, plus 100 per infusion, plus 1 per level.
pub fn weapon_level(id: u32) -> u32 {
    id % 100
}

pub fn weapon_infusion(id: u32) -> u32 {
    id % 10_000 / 100
}

pub fn pick_level(choice: WeaponTrait, max: u32, current: u32, roll: usize) -> u32 {
    match choice {
        WeaponTrait::Base => 0,
        WeaponTrait::Random => roll as u32 % (max + 1),
        WeaponTrait::SameAsRightHand => current.min(max),
    }
}

pub fn pick_infusion(choice: WeaponTrait, available: &[u32], current: u32, roll: usize) -> u32 {
    match choice {
        WeaponTrait::Base => 0,
        WeaponTrait::Random if available.is_empty() => 0,
        WeaponTrait::Random => available[roll % available.len()],
        WeaponTrait::SameAsRightHand if available.contains(&current) => current,
        WeaponTrait::SameAsRightHand => 0,
    }
}

/// Requirements and stats are strength, dexterity, intelligence, faith.
pub fn usable_two_handed(stats: [u32; 4], requirements: [u8; 4]) -> bool {
    stats[0] * 3 / 2 >= requirements[0] as u32
        && stats[1] >= requirements[1] as u32
        && stats[2] >= requirements[2] as u32
        && stats[3] >= requirements[3] as u32
}

/// Returns the final weapon id and how many weapons it was picked from.
fn choose_weapon(options: &WeaponOptions, current: u32, stats: [u32; 4]) -> Result<(u32, usize), String> {
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the weapon list is unavailable: {error:?}"))?;
    let weapons = regulation.get_param::<EQUIP_PARAM_WEAPON_ST>();
    let requirements = |row: &EQUIP_PARAM_WEAPON_ST| {
        [row.proper_strength(), row.proper_agility(), row.proper_magic(), row.proper_faith()]
    };

    // Unknown DLC status counts as not installed, which only makes the pool smaller.
    let (ashes, ringed_city) = unsafe { darksouls3::cs::CSDlc::instance() }
        .map(|dlc| (dlc.dlc1_installed, dlc.dlc2_installed))
        .unwrap_or((false, false));

    let mut candidates = Vec::new();
    for (id, row) in weapons.iter() {
        if !is_base_weapon_id(id) || row.sort_id() <= 0 {
            continue;
        }
        let base = id as u32;
        if !options.include_missing_dlc && !weapon_available(base, ashes, ringed_city) {
            continue;
        }
        let available: Vec<u32> = (0..INFUSION_COUNT)
            .filter(|infusion| weapons.get(u64::from(base + infusion * 100)).is_some())
            .collect();
        let infusion = pick_infusion(
            options.infusion,
            &available,
            weapon_infusion(current),
            random_index(INFUSION_COUNT as usize),
        );
        let variant = base + infusion * 100;
        let Some(variant_row) = weapons.get(u64::from(variant)) else {
            continue;
        };
        if !options.usable_only || usable_two_handed(stats, requirements(variant_row)) {
            candidates.push(variant);
        }
    }
    // Choosing the weapon already held would change nothing.
    let held = current / 10_000;
    if candidates.iter().any(|variant| variant / 10_000 != held) {
        candidates.retain(|variant| variant / 10_000 != held);
    }
    if candidates.is_empty() {
        return Err("no weapon in the game data fits these options".to_string());
    }

    let variant = candidates[random_index(candidates.len())];
    let row = weapons
        .get(u64::from(variant))
        .ok_or_else(|| format!("weapon {variant} disappeared from the game data"))?;
    let reinforce = regulation.get_param::<REINFORCE_PARAM_WEAPON_ST>();
    let reinforce_type = u64::from(row.reinforce_type_id().max(0) as u16);
    let max_level = (1..=MAX_WEAPON_LEVEL)
        .take_while(|level| reinforce.get(reinforce_type + u64::from(*level)).is_some())
        .count() as u32;
    let level = pick_level(
        options.level,
        max_level,
        weapon_level(current),
        random_index(MAX_WEAPON_LEVEL as usize + 1),
    );
    Ok((variant + level, candidates.len()))
}

/// Calls the menu routine for `slot`. A negative `index` empties the slot.
fn call_equip(player: &mut PlayerIns, slot: u32, index: i32) -> Result<(), String> {
    let equip_item = *cached()?.equip_item.as_ref().map_err(|error| error.clone())?;
    let mut buffer = [0u8; EQUIP_BUFFER_SIZE];
    let (offset, value) = if index < 0 {
        (EQUIP_BUFFER_UNEQUIP, -1)
    } else {
        (EQUIP_BUFFER_INVENTORY_INDEX, index)
    };
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    let equip: extern "C" fn(u32, *const u8) = unsafe { std::mem::transmute(equip_item) };

    let player_addr = player as *mut PlayerIns as usize;
    let flags_address = module_pointer(player_addr, EQUIP_UNLOCK_MODULE_OFFSET)?
        .wrapping_add(EQUIP_UNLOCK_FLAGS_OFFSET);
    let flags = read_u32_checked(flags_address)?;
    write_u32_checked(flags_address, flags | 1)?;
    equip(slot, buffer.as_ptr());
    write_u32_checked(flags_address, flags)
}

fn is_armor(entry: &darksouls3::sprj::EquipInventoryDataListEntry, piece: u32) -> bool {
    entry.item_id.category() as u32 == ItemCategory::Protector as u32 && entry.item_id.param_id() == piece
}

fn count_armor(player: &PlayerIns, piece: u32) -> u32 {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    equipment
        .equip_inventory_data
        .items_data
        .items()
        .filter(|entry| is_armor(entry, piece))
        .count() as u32
}

/// Copies of an armor piece are identical, so any copy that is not being worn will do.
fn spare_armor_index(player: &PlayerIns, piece: u32) -> Option<i32> {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    let inventory = &equipment.equip_inventory_data;
    (0..=u32::try_from(inventory.max_item_index).ok()?)
        .find(|index| {
            inventory.items_data[*index].as_option().is_some_and(|entry| is_armor(entry, piece))
                && !equipment.equipment_indexes.contains(&(*index as i32))
        })
        .map(|index| index as i32)
}

fn holds_item(player: &PlayerIns, param: u32, index: i32) -> bool {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    u32::try_from(index)
        .ok()
        .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
        .is_some_and(|entry| entry.item_id.param_id() == param)
}

/// Where an item is now. A save can come back with its inventory in a different order,
/// so a remembered index is only trusted if it still holds the same item.
fn current_index(player: &mut PlayerIns, category: ItemCategory, param: u32, index: i32) -> Option<i32> {
    if holds_item(player, param, index) {
        return Some(index);
    }
    let equipment = unsafe { &mut player.player_game_data.as_mut().equipment };
    let raw_id = ItemId::new(category, param).ok()?.into_inner();
    let find_item = *cached().ok()?.find_item.as_ref().ok()?;
    let find: extern "C" fn(*mut std::ffi::c_void, *const u32) -> i32 =
        unsafe { std::mem::transmute(find_item) };
    let inventory = &mut equipment.equip_inventory_data as *mut _ as *mut std::ffi::c_void;
    let found = find(inventory, &raw_id);
    (found >= 0).then_some(found)
}

/// Gives one of `item` and equips it through the menu routine. Returns its inventory
/// index once the slot is confirmed to hold it.
fn give_and_equip(player: &mut PlayerIns, item: ItemId, slot: u32) -> Result<i32, String> {
    let find_item = *cached()?.find_item.as_ref().map_err(|error| error.clone())?;
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    game_data.give_item_directly(item, 1);

    let raw_id = item.into_inner();
    let equipment = unsafe { &mut player.player_game_data.as_mut().equipment };
    let find: extern "C" fn(*mut std::ffi::c_void, *const u32) -> i32 =
        unsafe { std::mem::transmute(find_item) };
    let inventory = &mut equipment.equip_inventory_data as *mut _ as *mut std::ffi::c_void;
    let index = find(inventory, &raw_id);
    if index < 0 {
        return Err(format!("item {raw_id:#X} was not found in the inventory after giving it"));
    }

    call_equip(player, slot, index)?;

    let equipped = equipment.equipment_indexes[slot as usize];
    if equipped != index {
        return Err(format!(
            "item {raw_id:#X} was added, but the game did not equip it (slot {slot} holds {equipped}, wanted {index})"
        ));
    }
    Ok(index)
}

/// Deletes an inventory entry unless it is empty or still equipped in any slot.
/// Returns whether the entry is gone. The game's return value is not a success flag.
fn remove_inventory_entry(player: &mut PlayerIns, index: i32) -> Result<bool, String> {
    let equipment = unsafe { &mut player.player_game_data.as_mut().equipment };
    if index < 0 || equipment.equipment_indexes.contains(&index) {
        return Ok(false);
    }
    let address = *cached()?.remove_item.as_ref().map_err(|error| error.clone())?;
    let remove: extern "C" fn(*mut std::ffi::c_void, i32, u8, u8) -> u8 =
        unsafe { std::mem::transmute(address) };
    let target = equipment as *mut _ as *mut std::ffi::c_void;
    remove(target, index, 1, 1);
    Ok(equipment.equip_inventory_data.items_data[index as u32].as_option().is_none())
}

pub struct ArmorOptions {
    pub remove_old: bool,
    pub include_missing_dlc: bool,
    pub style: ArmorStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorStyle {
    RandomPieces,
    MatchingSet,
    EitherAtRandom,
}

impl ArmorStyle {
    pub fn from_index(index: i64) -> Self {
        match index {
            1 => Self::MatchingSet,
            2 => Self::EitherAtRandom,
            _ => Self::RandomPieces,
        }
    }

    /// `roll` only matters for [Self::EitherAtRandom], where odd means a set.
    pub fn uses_set(self, roll: usize) -> bool {
        match self {
            Self::RandomPieces => false,
            Self::MatchingSet => true,
            Self::EitherAtRandom => roll % 2 == 1,
        }
    }
}

/// Head, body, hands, legs.
const ARMOR_SLOTS: [u32; 4] = [0xC, 0xD, 0xE, 0xF];
const ASHES_OF_ARIANDEL_ARMOR: [u32; 19] = [
    20_000_000, 20_001_000, 20_002_000, 20_003_000, 55_500_000, 55_501_000, 55_502_000,
    55_503_000, 56_500_000, 56_501_000, 56_503_000, 58_500_000, 58_501_000, 58_502_000,
    58_503_000, 60_500_000, 60_501_000, 60_502_000, 60_503_000
];
const RINGED_CITY_ARMOR: [u32; 37] = [
    28_501_000, 28_502_000, 28_503_000, 29_500_000, 29_501_000, 29_502_000, 29_503_000,
    32_000_000, 32_001_000, 32_002_000, 32_003_000, 63_500_000, 63_501_000, 63_502_000,
    63_503_000, 64_501_000, 64_502_000, 64_503_000, 67_500_000, 67_501_000, 67_502_000,
    67_503_000, 68_500_000, 80_500_000, 80_501_000, 80_502_000, 80_503_000, 81_500_000,
    81_501_000, 81_502_000, 81_503_000, 82_500_000, 82_501_000, 82_502_000, 82_503_000,
    83_500_000, 84_500_000
];

pub fn is_listed_armor(id: u32) -> bool {
    crate::armor_ids::REAL_ARMOR.binary_search(&id).is_ok()
}

/// Empty armor slots hold placeholder pieces with ids below this.
pub fn is_real_armor(id: u32) -> bool {
    id >= 1_000_000
}

pub fn armor_available(id: u32, ashes_installed: bool, ringed_city_installed: bool) -> bool {
    (ashes_installed || !ASHES_OF_ARIANDEL_ARMOR.contains(&id))
        && (ringed_city_installed || !RINGED_CITY_ARMOR.contains(&id))
}

/// Armor ids for each of [ARMOR_SLOTS], taken from the game's data.
fn armor_pools(include_missing_dlc: bool) -> Result<[Vec<u32>; 4], String> {
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the armor list is unavailable: {error:?}"))?;
    let (ashes, ringed_city) = unsafe { darksouls3::cs::CSDlc::instance() }
        .map(|dlc| (dlc.dlc1_installed, dlc.dlc2_installed))
        .unwrap_or((false, false));
    let mut pools: [Vec<u32>; 4] = Default::default();
    for (id, row) in regulation.get_param::<EQUIP_PARAM_PROTECTOR_ST>().iter() {
        let id = id as u32;
        if !is_listed_armor(id) {
            continue;
        }
        if !include_missing_dlc && !armor_available(id, ashes, ringed_city) {
            continue;
        }
        let slots = [row.head_equip(), row.body_equip(), row.arm_equip(), row.leg_equip()];
        if let Some(slot) = slots.iter().position(|fits| *fits) {
            pools[slot].push(id);
        }
    }
    Ok(pools)
}

/// Groups the pools into sets. Pieces of a set share an id apart from the thousands
/// digit. Lone pieces, such as a single mask, are not treated as sets.
pub fn armor_sets(pools: &[Vec<u32>; 4]) -> Vec<[Option<u32>; 4]> {
    let mut sets = std::collections::BTreeMap::<u32, [Option<u32>; 4]>::new();
    for (slot, pool) in pools.iter().enumerate() {
        for id in pool {
            sets.entry(id / 10_000).or_default()[slot] = Some(*id);
        }
    }
    sets.into_values()
        .filter(|set| set.iter().flatten().count() >= 2)
        .collect()
}

/// One slot changed by [roll_armor], with what it needs to be put back.
#[derive(Deserialize, Serialize)]
struct ArmorSwap {
    slot: u32,
    old_index: i32,
    old_id: u32,
    /// Copies of `piece` before the swap, because the temporary copy goes back by count.
    owned_before: u32,
    piece: u32,
}

struct ArmorRoll {
    swaps: Vec<ArmorSwap>,
    failure: Option<String>,
    used_set: bool,
}

/// Gives and equips a new piece for each armor slot.
fn roll_armor(
    player: &mut PlayerIns,
    style: ArmorStyle,
    include_missing_dlc: bool,
) -> Result<ArmorRoll, String> {
    let pools = armor_pools(include_missing_dlc)?;
    let set = if style.uses_set(random_index(2)) {
        let sets = armor_sets(&pools);
        if sets.is_empty() {
            return Err("no armor sets were found in the game data".to_string());
        }
        Some(sets[random_index(sets.len())])
    } else {
        None
    };

    let mut swaps = Vec::new();
    let mut failure = None;
    for (position, (slot, pool)) in ARMOR_SLOTS.into_iter().zip(&pools).enumerate() {
        if pool.is_empty() {
            continue;
        }
        let player_data = unsafe { player.player_game_data.as_ref() };
        let old_index = player_data.equipment.equipment_indexes[slot as usize];
        let old_id = u32::try_from(old_index)
            .ok()
            .and_then(|index| {
                player_data.equipment.equip_inventory_data.items_data[index]
                    .as_option()
                    .map(|entry| entry.item_id.param_id())
            })
            .unwrap_or(0);

        // Choosing the piece already worn would change nothing.
        let choices: Vec<u32> = match set {
            Some(set) => set[position].into_iter().collect(),
            None => pool.clone(),
        };
        let choices: Vec<u32> = choices.into_iter().filter(|id| *id != old_id).collect();
        let Some(&piece) = choices.get(random_index(choices.len())) else {
            continue;
        };
        let item = ItemId::new(ItemCategory::Protector, piece)
            .map_err(|error| format!("armor {piece} has an invalid id: {error:?}"))?;

        let owned_before = count_armor(player, piece);
        match give_and_equip(player, item, slot) {
            Ok(_) => swaps.push(ArmorSwap { slot, old_index, old_id, owned_before, piece }),
            Err(error) => failure = Some(error),
        }
    }
    Ok(ArmorRoll { swaps, failure, used_set: set.is_some() })
}

pub fn random_armor(options: ArmorOptions) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    if pending().bombardier.is_some() {
        return Err(BOMBARDIER_ACTIVE.to_string());
    }
    let roll = roll_armor(player, options.style, options.include_missing_dlc)?;

    let mut removed = 0;
    if options.remove_old {
        for swap in &roll.swaps {
            if is_real_armor(swap.old_id) && remove_inventory_entry(player, swap.old_index)? {
                removed += 1;
            }
        }
    }
    let equipped: Vec<u32> = roll.swaps.iter().map(|swap| swap.piece).collect();
    match roll.failure {
        Some(error) if equipped.is_empty() => Err(error),
        Some(error) => Ok(format!("equipped armor {equipped:?}, but one piece failed: {error}")),
        None if equipped.is_empty() => Err("no armor was found in the game data".to_string()),
        None => {
            let style = if roll.used_set { "set" } else { "pieces" };
            Ok(format!("equipped armor {style} {equipped:?}, removed {removed} old pieces"))
        }
    }
}

#[derive(Deserialize, Serialize)]
struct TemporaryArmor {
    until: u64,
    character: String,
    swaps: Vec<ArmorSwap>,
}

const PENDING_RESTORES_FILE: &str = "pending-restores.json";
const OTHER_CHARACTER_WAITING: &str = "another character is still waiting to get its gear or stats back";

/// Everything a temporary swap must put back. It lives in a file as well, so a crash or
/// a quit cannot leave a character stuck with the swap.
#[derive(Default, Deserialize, Serialize)]
struct PendingRestores {
    armor: Option<TemporaryArmor>,
    bombardier: Option<Bombardier>,
    stats: Option<ShuffledStats>,
}

impl PendingRestores {
    fn is_empty(&self) -> bool {
        self.armor.is_none() && self.bombardier.is_none() && self.stats.is_none()
    }
}

static PENDING: Mutex<PendingRestores> =
    Mutex::new(PendingRestores { armor: None, bombardier: None, stats: None });

fn pending() -> MutexGuard<'static, PendingRestores> {
    PENDING.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Wall-clock time, because the end of a swap has to mean something after a restart.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn write_pending(restores: &PendingRestores) -> Result<(), String> {
    let path = crate::log::app_dir()?.join(PENDING_RESTORES_FILE);
    if restores.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_string_pretty(restores).map_err(|error| error.to_string())?;
    // Written beside the real file first, so a crash mid-write cannot lose the old one.
    let partial = path.with_extension("tmp");
    std::fs::write(&partial, text).map_err(|error| error.to_string())?;
    std::fs::rename(&partial, &path).map_err(|error| error.to_string())
}

fn save_pending(restores: &PendingRestores) {
    if let Err(error) = write_pending(restores) {
        crate::log::log(format!("could not save pending restores: {error}"));
    }
}

/// Picks up swaps from before a crash or a quit. The polls below finish them once that
/// character is back in the world.
pub fn load_pending_restores() {
    let Ok(dir) = crate::log::app_dir() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(dir.join(PENDING_RESTORES_FILE)) else {
        return;
    };
    match serde_json::from_str::<PendingRestores>(&text) {
        Ok(restores) => {
            *pending() = restores;
        }
        Err(error) => crate::log::log(format!("pending restores could not be read: {error}")),
    }
}

pub fn temporary_armor(
    seconds: i32,
    style: ArmorStyle,
    include_missing_dlc: bool,
) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let until = unix_now() + seconds.max(0) as u64;
    let character = read_character().1;
    let mut slot = pending();
    if slot.bombardier.is_some() {
        return Err(BOMBARDIER_ACTIVE.to_string());
    }
    if let Some(active) = slot.armor.as_mut() {
        if active.character != character {
            return Err(OTHER_CHARACTER_WAITING.to_string());
        }
        // A second outfit would lose track of the armor to put back.
        active.until = active.until.max(until);
        save_pending(&slot);
        return Ok(format!("temporary armor extended to at least {seconds} more seconds"));
    }
    let roll = roll_armor(player, style, include_missing_dlc)?;
    if roll.swaps.is_empty() {
        return Err(roll.failure.unwrap_or_else(|| "no armor was found in the game data".to_string()));
    }
    let equipped: Vec<u32> = roll.swaps.iter().map(|swap| swap.piece).collect();
    slot.armor = Some(TemporaryArmor { until, character, swaps: roll.swaps });
    save_pending(&slot);
    Ok(format!("temporary armor {equipped:?} for {seconds} seconds"))
}

/// Armor goes back only while the player is in the world and not mid-death.
pub fn can_restore_armor(load_screen: bool, player_loaded: bool, alive: bool, fake_death: bool) -> bool {
    !load_screen && player_loaded && alive && !fake_death
}

fn restore_armor_slot(player: &mut PlayerIns, swap: &ArmorSwap) -> Result<(), String> {
    let owned = current_index(player, ItemCategory::Protector, swap.old_id, swap.old_index);
    let target = owned.filter(|_| is_real_armor(swap.old_id)).unwrap_or(-1);
    call_equip(player, swap.slot, target)?;

    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    let equipped = equipment.equipment_indexes[swap.slot as usize];
    if target >= 0 && equipped != target {
        return Err(format!("slot {} holds {equipped}, wanted {target}", swap.slot));
    }
    // After a crash before a save the count is unchanged, so nothing is removed. One copy
    // at most goes, so a copy picked up during the swap stays.
    if count_armor(player, swap.piece) > swap.owned_before {
        if let Some(index) = spare_armor_index(player, swap.piece) {
            remove_inventory_entry(player, index)?;
        }
    }
    Ok(())
}

pub fn poll_temporary_armor() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = pending();
        let Some(active) = slot.armor.as_ref() else {
            return;
        };
        if unix_now() < active.until {
            return;
        }
        let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        let player = local_player();
        let alive = player
            .as_ref()
            .is_ok_and(|player| unsafe { &**player }.super_chr_ins.modules.data.hp > 0);
        if !can_restore_armor(load_screen_open(), player.is_ok(), alive, fake_death) {
            return;
        }
        let (loaded, character) = read_character();
        let Ok(player) = player else {
            return;
        };
        if !loaded || character != active.character {
            return;
        }
        let Some(active) = slot.armor.take() else {
            return;
        };
        let player = unsafe { &mut *player };
        for swap in &active.swaps {
            if let Err(error) = restore_armor_slot(player, swap) {
                crate::log::log(format!("temporary armor slot {} restore failed: {error}", swap.slot));
            }
        }
        save_pending(&slot);
    }));
}

/// Left and right hands, three weapons each.
const WEAPON_SLOTS: [u32; 6] = [0, 1, 2, 3, 4, 5];
const BLACK_FIREBOMB: u32 = 0x129;
/// How a quick slot stores the item: the category in the top bits, then the param id.
const BLACK_FIREBOMB_SLOT_VALUE: i32 = 0x4000_0129;
/// The game's own limit. A stack above it spills onto the floor, so the event stays at it
/// and refills as bombs are thrown.
const BOMB_COUNT: u32 = 10;
/// The Mirrah Chain Set, in [ARMOR_SLOTS] order.
const BOMBARDIER_ARMOR: [u32; 4] = [79_000_000, 79_001_000, 79_002_000, 79_003_000];
const BOMBARDIER_ACTIVE: &str = "Bombardier is using the armor slots";
/// The equip routine numbers the quick slots from here, after the gear slots.
const QUICK_EQUIP_SLOT: u32 = 0x16;
/// The real quick slots: a gaitem handle then an inventory index, 8 bytes each. The ids at
/// [QUICK_ITEM_OFFSET] are only a copy of what these hold.
const QUICK_PAIRS_OFFSET: usize = 0x490;
const ESTUS_PARAMS: std::ops::RangeInclusive<u32> = 190..=220;
const GOODS_CATEGORY: u32 = 4;
const ITEM_PARAM_MASK: u32 = 0x0FFF_FFFF;

#[derive(Deserialize, Serialize)]
struct RemovedWeapon {
    slot: u32,
    index: i32,
    weapon: u32,
}

#[derive(Deserialize, Serialize)]
struct Bombardier {
    until: u64,
    character: String,
    weapons: Vec<RemovedWeapon>,
    /// Saved swaps from before the armor was added have no such field.
    #[serde(default)]
    armor: Vec<ArmorSwap>,
    bombs_before: u32,
    quick_slot: usize,
    quick_item: i32,
    quick_index: i32,
    selected_before: i32,
    #[serde(skip)]
    selection_settled: bool,
    #[serde(skip)]
    weapons_changed: bool,
}

fn black_firebomb() -> Result<ItemId, String> {
    ItemId::new(ItemCategory::Goods, BLACK_FIREBOMB)
        .map_err(|error| format!("the Black Firebomb id is invalid: {error:?}"))
}

fn write_i32(pgd: *mut PlayerGameData, offset: usize, value: i32) -> Result<(), String> {
    if pgd.is_null() {
        return Err("player data is missing".to_string());
    }
    unsafe { pgd.cast::<u8>().add(offset).cast::<i32>().write(value) };
    Ok(())
}

pub fn is_estus(item: i32) -> bool {
    let item = item as u32;
    item >> 28 == GOODS_CATEGORY && ESTUS_PARAMS.contains(&(item & ITEM_PARAM_MASK))
}

/// A slot already holding the bombs, then a free slot, then anything that is not Estus.
pub fn pick_bomb_slot(items: &[i32]) -> Option<usize> {
    items
        .iter()
        .position(|item| *item == BLACK_FIREBOMB_SLOT_VALUE)
        .or_else(|| items.iter().position(|item| is_empty_item(*item)))
        .or_else(|| items.iter().position(|item| !is_estus(*item)))
}

/// Equips like the menu does, which takes off whatever the slot held. A negative `index`
/// empties it. If the game ignores the request, the slot is written directly.
fn equip_quick_item(player: &mut PlayerIns, slot: usize, index: i32, item: i32) -> Result<(), String> {
    call_equip(player, QUICK_EQUIP_SLOT + slot as u32, index)?;
    let pgd = player.player_game_data.as_ptr();
    let pair = QUICK_PAIRS_OFFSET + slot * 8;
    if read_i32(pgd, pair + 4)? == index {
        return Ok(());
    }
    crate::log::log(format!("quick slot {slot}: the equip routine did not take it, writing directly"));
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    let handle = u32::try_from(index)
        .ok()
        .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
        .map_or(0, |entry| entry.gaitem_handle.get());
    write_i32(pgd, pair, handle as i32)?;
    write_i32(pgd, pair + 4, index)?;
    write_i32(pgd, QUICK_ITEM_OFFSET + slot * 4, item)
}

fn set_bomb_count(pgd: *mut PlayerGameData, count: u32) -> Result<(), String> {
    set_stack(pgd, count)?;
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    let main = &mut *game_data.main_player_game_data as *mut PlayerGameData;
    if main != pgd {
        set_stack(main, count)?;
    }
    Ok(())
}

fn set_stack(pgd: *mut PlayerGameData, count: u32) -> Result<(), String> {
    let data = unsafe { pgd.as_mut() }.ok_or("player data is missing")?;
    data.equipment
        .equip_inventory_data
        .items_data
        .items_mut()
        .find(|entry| item_matches(entry.item_id, BLACK_FIREBOMB_SLOT_VALUE))
        .map(|entry| entry.quantity = count)
        .ok_or_else(|| "the Black Firebombs did not reach the inventory".to_string())
}

/// Wears the set, and returns what each slot needs to get its own armor back.
fn wear_bombardier_armor(player: &mut PlayerIns) -> Vec<ArmorSwap> {
    let mut swaps = Vec::new();
    for (slot, piece) in ARMOR_SLOTS.into_iter().zip(BOMBARDIER_ARMOR) {
        let equipment = unsafe { &player.player_game_data.as_ref().equipment };
        let old_index = equipment.equipment_indexes[slot as usize];
        let old_id = u32::try_from(old_index)
            .ok()
            .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
            .map_or(0, |entry| entry.item_id.param_id());
        if old_id == piece {
            continue;
        }
        let owned_before = count_armor(player, piece);
        let equipped = ItemId::new(ItemCategory::Protector, piece)
            .map_err(|error| format!("armor {piece} has an invalid id: {error:?}"))
            .and_then(|item| give_and_equip(player, item, slot));
        match equipped {
            Ok(_) => swaps.push(ArmorSwap { slot, old_index, old_id, owned_before, piece }),
            Err(error) => crate::log::log(format!("Bombardier: armor slot {slot} failed: {error}")),
        }
    }
    swaps
}

/// Leaves any bombs the player owned before, even if some of the event's bombs are still unused.
pub fn bombs_to_remove(remaining: u32, before: u32) -> u32 {
    remaining.saturating_sub(before)
}

pub fn bombardier(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let until = unix_now() + seconds as u64;
    let character = read_character().1;
    let pgd = player.player_game_data.as_ptr();
    let mut slot = pending();
    if let Some(active) = slot.bombardier.as_mut() {
        if active.character != character {
            return Err(OTHER_CHARACTER_WAITING.to_string());
        }
        active.until = active.until.max(until);
        save_pending(&slot);
        return Ok(format!("Bombardier extended to at least {seconds} more seconds"));
    }

    let quick_items = (0..QUICK_ITEM_COUNT)
        .map(|quick_slot| quick_item_id(pgd, quick_slot))
        .collect::<Result<Vec<_>, _>>()?;
    let quick_slot = pick_bomb_slot(&quick_items).ok_or("every quick slot holds an Estus Flask")?;
    let quick_item = quick_items[quick_slot];
    let quick_index = read_i32(pgd, QUICK_PAIRS_OFFSET + quick_slot * 8 + 4)?;
    let selected_before = read_i32(pgd, SELECTED_QUICK_SLOT_OFFSET)?;
    let bombs_before = item_quantity(pgd, BLACK_FIREBOMB_SLOT_VALUE).unwrap_or(0);
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    // Only what is missing, because anything over the limit would land on the floor.
    let missing = BOMB_COUNT.saturating_sub(bombs_before);
    if missing > 0 {
        game_data.give_item_directly(black_firebomb()?, missing);
    }

    let mut weapons = Vec::new();
    for weapon_slot in WEAPON_SLOTS {
        let equipment = unsafe { &player.player_game_data.as_ref().equipment };
        let index = equipment.equipment_indexes[weapon_slot as usize];
        let weapon = u32::try_from(index)
            .ok()
            .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
            .map(|entry| entry.item_id.param_id());
        let Some(weapon) = weapon else {
            continue;
        };
        if !is_removable_weapon(weapon) {
            continue;
        }
        call_equip(player, weapon_slot, -1)?;
        weapons.push(RemovedWeapon { slot: weapon_slot, index, weapon });
    }
    // Temporary Armor already remembers the player's own armor, so a second swap on top
    // of it could lose track of what goes back.
    let armor = if slot.armor.is_some() { Vec::new() } else { wear_bombardier_armor(player) };
    let removed = weapons.len();
    slot.bombardier = Some(Bombardier {
        until,
        character,
        weapons,
        armor,
        bombs_before,
        quick_slot,
        quick_item,
        quick_index,
        selected_before,
        selection_settled: false,
        weapons_changed: false,
    });
    save_pending(&slot);
    let bomb_index = current_index(player, ItemCategory::Goods, BLACK_FIREBOMB, -1)
        .ok_or("the Black Firebombs did not reach the inventory")?;
    equip_quick_item(player, quick_slot, bomb_index, BLACK_FIREBOMB_SLOT_VALUE)?;
    write_i32(pgd, SELECTED_QUICK_SLOT_OFFSET, quick_slot as i32)?;
    Ok(format!(
        "Bombardier for {seconds} seconds: {removed} weapons removed, quick slot {quick_slot} holds {} bombs",
        item_quantity(pgd, BLACK_FIREBOMB_SLOT_VALUE).unwrap_or(0),
    ))
}

fn end_bombardier(player: &mut PlayerIns, active: &Bombardier) -> Result<(), String> {
    for removed in &active.weapons {
        match current_index(player, ItemCategory::Weapon, removed.weapon, removed.index) {
            Some(index) => call_equip(player, removed.slot, index)?,
            None => crate::log::log(format!("Bombardier: weapon {} is no longer owned", removed.weapon)),
        }
    }
    for swap in &active.armor {
        if let Err(error) = restore_armor_slot(player, swap) {
            crate::log::log(format!("Bombardier: armor slot {} restore failed: {error}", swap.slot));
        }
    }
    let param = active.quick_item as u32 & ITEM_PARAM_MASK;
    let target = if is_empty_item(active.quick_item) {
        -1
    } else {
        current_index(player, ItemCategory::Goods, param, active.quick_index).unwrap_or(-1)
    };
    let item = if target < 0 { -1 } else { active.quick_item };
    equip_quick_item(player, active.quick_slot, target, item)?;
    let pgd = player.player_game_data.as_ptr();
    write_i32(pgd, SELECTED_QUICK_SLOT_OFFSET, active.selected_before)?;
    let remaining = item_quantity(pgd, BLACK_FIREBOMB_SLOT_VALUE).unwrap_or(0);
    let extra = bombs_to_remove(remaining, active.bombs_before);
    if extra > 0 {
        let game_data = unsafe { GameDataMan::instance_mut() }
            .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
        game_data.remove_item(black_firebomb()?, extra);
    }
    Ok(())
}

/// The game can move the selection back by itself once after the bombs are equipped.
/// Only the first change is corrected, so the player can still switch to Estus after.
fn keep_bombs_selected(active: &mut Bombardier) {
    if active.selection_settled {
        return;
    }
    let Ok(player) = local_player() else {
        return;
    };
    let pgd = unsafe { &*player }.player_game_data.as_ptr();
    let Ok(selected) = read_i32(pgd, SELECTED_QUICK_SLOT_OFFSET) else {
        return;
    };
    if selected == active.quick_slot as i32 {
        return;
    }
    active.selection_settled = true;
    if crate::input::switching_item() {
        return;
    }
    let _ = write_i32(pgd, SELECTED_QUICK_SLOT_OFFSET, active.quick_slot as i32);
}

/// Tops the stack up each frame, so it never reaches 0 and leaves the quick slot. If the
/// bombs are gone anyway, for example discarded in the menu, they are given and equipped again.
fn keep_bombs_stocked(active: &Bombardier) -> Result<(), String> {
    let player = unsafe { &mut *local_player()? };
    let pgd = player.player_game_data.as_ptr();
    match item_quantity(pgd, BLACK_FIREBOMB_SLOT_VALUE) {
        Some(count) if count >= BOMB_COUNT => Ok(()),
        Some(_) => set_bomb_count(pgd, BOMB_COUNT),
        None => {
            let game_data = unsafe { GameDataMan::instance_mut() }
                .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
            game_data.give_item_directly(black_firebomb()?, BOMB_COUNT);
            let index = current_index(player, ItemCategory::Goods, BLACK_FIREBOMB, -1)
                .ok_or("the Black Firebombs did not reach the inventory")?;
            equip_quick_item(player, active.quick_slot, index, BLACK_FIREBOMB_SLOT_VALUE)
        }
    }
}

/// Weapons stay in the inventory, so anything equipped from the menu comes straight off.
/// The first time, it is remembered so that slot's own weapon is not lost at the end.
fn keep_hands_empty(active: &mut Bombardier) {
    let Ok(player) = local_player() else {
        return;
    };
    let player = unsafe { &mut *player };
    for weapon_slot in WEAPON_SLOTS {
        let equipment = unsafe { &player.player_game_data.as_ref().equipment };
        let index = equipment.equipment_indexes[weapon_slot as usize];
        let weapon = u32::try_from(index)
            .ok()
            .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
            .map(|entry| entry.item_id.param_id());
        let Some(weapon) = weapon.filter(|weapon| is_removable_weapon(*weapon)) else {
            continue;
        };
        if let Err(error) = call_equip(player, weapon_slot, -1) {
            crate::log::log(format!("Bombardier: could not clear slot {weapon_slot}: {error}"));
            continue;
        }
        if !active.weapons.iter().any(|removed| removed.slot == weapon_slot) {
            active.weapons.push(RemovedWeapon { slot: weapon_slot, index, weapon });
            active.weapons_changed = true;
        }
    }
}

pub fn poll_bombardier() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = pending();
        let Some(active) = slot.bombardier.as_mut() else {
            return;
        };
        keep_bombs_selected(active);
        if !load_screen_open() {
            keep_hands_empty(active);
            if unix_now() < active.until {
                let _ = keep_bombs_stocked(active);
            }
        }
        if std::mem::take(&mut active.weapons_changed) {
            save_pending(&slot);
        }
        let Some(active) = slot.bombardier.as_ref() else {
            return;
        };
        if unix_now() < active.until {
            return;
        }
        let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        let player = local_player();
        let alive = player
            .as_ref()
            .is_ok_and(|player| unsafe { &**player }.super_chr_ins.modules.data.hp > 0);
        if !can_restore_armor(load_screen_open(), player.is_ok(), alive, fake_death) {
            return;
        }
        let (loaded, character) = read_character();
        let Ok(player) = player else {
            return;
        };
        if !loaded || character != active.character {
            return;
        }
        let Some(active) = slot.bombardier.take() else {
            return;
        };
        if let Err(error) = end_bombardier(unsafe { &mut *player }, &active) {
            crate::log::log(format!("Bombardier restore failed: {error}"));
        }
        save_pending(&slot);
    }));
}

const STAT_COUNT: usize = 9;
const STAT_NAMES: [&str; STAT_COUNT] = ["Vig", "Att", "End", "Vit", "Str", "Dex", "Int", "Fth", "Lck"];
const MAX_SHUFFLE_ROLLS: usize = 20;

#[derive(Deserialize, Serialize)]
struct ShuffledStats {
    until: u64,
    character: String,
    original: [u32; STAT_COUNT],
}


fn read_stats(info: &PlayerInfo) -> [u32; STAT_COUNT] {
    [
        info.vigor,
        info.attunement,
        info.endurance,
        info.vitality,
        info.strength,
        info.dexterity,
        info.intelligence,
        info.faith,
        info.luck,
    ]
}

fn write_stats(info: &mut PlayerInfo, stats: [u32; STAT_COUNT]) {
    let [vigor, attunement, endurance, vitality, strength, dexterity, intelligence, faith, luck] = stats;
    info.vigor = vigor;
    info.attunement = attunement;
    info.endurance = endurance;
    info.vitality = vitality;
    info.strength = strength;
    info.dexterity = dexterity;
    info.intelligence = intelligence;
    info.faith = faith;
    info.luck = luck;
}

fn apply_stats(player: &mut PlayerIns, stats: [u32; STAT_COUNT]) -> Result<(), String> {
    write_stats(unsafe { &mut player.player_game_data.as_mut().player_info }, stats);
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    write_stats(&mut game_data.main_player_game_data.player_info, stats);
    Ok(())
}

/// Fisher-Yates, so every stat value is kept and only their places change.
pub fn shuffled_stats(mut stats: [u32; STAT_COUNT], mut roll: impl FnMut(usize) -> usize) -> [u32; STAT_COUNT] {
    for index in (1..STAT_COUNT).rev() {
        stats.swap(index, roll(index + 1));
    }
    stats
}

/// A shuffle keeps the total, so a different total means the player levelled up or the
/// game loaded older stats. Putting the remembered ones back would then lose levels.
pub fn can_restore_stats(current: [u32; STAT_COUNT], original: [u32; STAT_COUNT]) -> bool {
    current.iter().sum::<u32>() == original.iter().sum::<u32>()
}

fn describe_stats(stats: [u32; STAT_COUNT]) -> String {
    STAT_NAMES
        .iter()
        .zip(stats)
        .map(|(name, value)| format!("{name} {value}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn shuffle_stats(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let current = read_stats(unsafe { &player.player_game_data.as_ref().player_info });
    let shuffled = (0..MAX_SHUFFLE_ROLLS)
        .map(|_| shuffled_stats(current, random_index))
        .find(|stats| *stats != current)
        .ok_or_else(|| "every stat has the same value, so there is nothing to shuffle".to_string())?;
    let character = read_character().1;
    let mut slot = pending();
    if slot.stats.as_ref().is_some_and(|active| active.character != character) {
        return Err(OTHER_CHARACTER_WAITING.to_string());
    }
    if seconds == 0 {
        slot.stats = None;
    } else {
        let until = unix_now() + seconds as u64;
        match slot.stats.as_mut() {
            // Shuffling again must still bring back the real stats, not the last shuffle.
            Some(active) => active.until = active.until.max(until),
            None => slot.stats = Some(ShuffledStats { until, character, original: current }),
        }
    }
    apply_stats(player, shuffled)?;
    save_pending(&slot);
    Ok(format!("stats shuffled: {}", describe_stats(shuffled)))
}

pub fn poll_shuffled_stats() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = pending();
        let Some(active) = slot.stats.as_ref() else {
            return;
        };
        if unix_now() < active.until {
            return;
        }
        let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        let player = local_player();
        let alive = player
            .as_ref()
            .is_ok_and(|player| unsafe { &**player }.super_chr_ins.modules.data.hp > 0);
        if !can_restore_armor(load_screen_open(), player.is_ok(), alive, fake_death) {
            return;
        }
        let (loaded, character) = read_character();
        let Ok(player) = player else {
            return;
        };
        if !loaded || character != active.character {
            return;
        }
        let Some(active) = slot.stats.take() else {
            return;
        };
        let player = unsafe { &mut *player };
        let current = read_stats(unsafe { &player.player_game_data.as_ref().player_info });
        if can_restore_stats(current, active.original) {
            if let Err(error) = apply_stats(player, active.original) {
                crate::log::log(format!("stat shuffle restore failed: {error}"));
            }
        } else {
            crate::log::log("stat shuffle not restored: the stat total changed");
        }
        save_pending(&slot);
    }));
}

pub fn random_weapon(options: WeaponOptions) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    if pending().bombardier.is_some() {
        return Err("Bombardier has taken the weapons away".to_string());
    }
    let player_data = unsafe { player.player_game_data.as_ref() };
    let info = &player_data.player_info;
    let stats = [info.strength, info.dexterity, info.intelligence, info.faith];
    let slot_index = player_data.equipment.equipment_indexes[RIGHT_HAND_SLOT as usize];
    let current = u32::try_from(slot_index)
        .ok()
        .and_then(|index| {
            player_data.equipment.equip_inventory_data.items_data[index]
                .as_option()
                .map(|entry| entry.item_id.param_id())
        })
        .unwrap_or(0);
    let (weapon, pool) = choose_weapon(&options, current, stats)?;
    let item = ItemId::new(ItemCategory::Weapon, weapon)
        .map_err(|error| format!("weapon {weapon} has an invalid id: {error:?}"))?;

    give_and_equip(player, item, RIGHT_HAND_SLOT)?;
    let mut removal = "";
    if options.remove_old
        && is_removable_weapon(current)
        && remove_inventory_entry(player, slot_index)?
    {
        removal = ", removed the old weapon";
    }
    Ok(format!(
        "equipped weapon {weapon} (+{}, infusion {}), picked from {pool} weapons{removal}",
        weapon_level(weapon),
        weapon_infusion(weapon),
    ))
}

pub fn set_health_percent(percent: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    let data = &mut player.super_chr_ins.modules.data;
    if data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    if data.max_hp <= 0 {
        return Err(format!("maximum HP is invalid ({})", data.max_hp));
    }
    let percent = percent.clamp(1, 100);
    let before = data.hp;
    let target = health_at_percent(data.max_hp, percent);
    data.hp = target;
    Ok(format!(
        "player HP {before} -> {target} ({percent}% of {})",
        data.max_hp
    ))
}

pub fn set_health_random_percent() -> Result<String, String> {
    set_health_percent(random_health_percent())
}

fn random_health_percent() -> i32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    let mut value = STATE.load(Ordering::Relaxed);
    if value == 0 {
        value = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
    }
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    STATE.store(value, Ordering::Relaxed);
    (value % 100 + 1) as i32
}

pub fn use_selected_item() -> Result<String, String> {
    let pgd = player_game_data()?;
    let slot = selected_quick_slot(pgd)?;
    let item_id = quick_item_id(pgd, slot)?;
    if is_empty_item(item_id) {
        return Err(format!("quick slot {slot} is empty"));
    }
    if item_quantity(pgd, item_id) == Some(0) {
        return Err(format!("quick slot {slot} item {item_id:#X} has quantity 0"));
    }
    let menu_open = menus_open()?;
    let sent = crate::input::queue_use_item(menu_open)?;
    Ok(sent)
}

pub fn menus_open() -> Result<bool, String> {
    let new_menu = unsafe { NewMenuSystem::instance() }
        .map_err(|error| format!("menu state is unavailable: {error:?}"))?;
    let menu_man = unsafe { MenuMan::instance() }
        .map_err(|error| format!("menu mode is unavailable: {error:?}"))?;
    Ok(new_menu.is_menu_open() || menu_man.is_menu_mode())
}

pub fn dump_player_data(soul_count: i64) -> Result<String, String> {
    let pgd = player_game_data()?;
    let bytes = unsafe { std::slice::from_raw_parts(pgd.cast::<u8>(), 0x950) };
    let dir = crate::log::app_dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("player-dump-{stamp}.txt"));
    let mut text = format!(
        "reportedSoulCount={soul_count}\nheldSoulsOffset={HELD_SOULS_OFFSET:#X}\nheldSoulsValue={}\nselectedSlotOffset={SELECTED_QUICK_SLOT_OFFSET:#X}\nselectedSlotRaw={}\n",
        read_souls(pgd).unwrap_or(-1),
        read_i32(pgd, SELECTED_QUICK_SLOT_OFFSET).unwrap_or(-1),
    );
    text.push_str("quickItems=");
    for slot in 0..QUICK_ITEM_COUNT {
        let id = quick_item_id(pgd, slot).unwrap_or(-1);
        text.push_str(&format!("{id:#X} "));
    }
    text.push_str("\nbytes:\n");
    for (index, chunk) in bytes.chunks(16).enumerate() {
        text.push_str(&format!("{:04X} ", index * 16));
        for byte in chunk {
            text.push_str(&format!("{byte:02X} "));
        }
        text.push('\n');
    }
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(format!("wrote {}", path.display()))
}

pub fn is_empty_item(item_id: i32) -> bool {
    item_id <= 0
}

pub fn classify_slot(raw: i32) -> Result<usize, ()> {
    if (0..QUICK_ITEM_COUNT as i32).contains(&raw) {
        Ok(raw as usize)
    } else {
        Err(())
    }
}

fn read_offline() -> (bool, bool) {
    match game_man() {
        Ok(game_man) => {
            let flag = unsafe { game_man.add(ONLINE_FLAG_OFFSET).read() };
            (true, flag == 0)
        }
        Err(_) => (false, false),
    }
}

fn read_character() -> (bool, String) {
    let Ok(player) = (unsafe { PlayerIns::local_player() }) else {
        return (false, String::new());
    };
    let pgd = unsafe { player.player_game_data.as_ref() };
    if pgd.equipment.is_main_menu() {
        return (false, String::new());
    }
    (true, pgd.player_info.name())
}

fn player_game_data() -> Result<*mut PlayerGameData, String> {
    let player = local_player()?;
    Ok(unsafe { (*player).player_game_data.as_ptr() })
}

fn local_player() -> Result<*mut PlayerIns, String> {
    unsafe { PlayerIns::local_player() }
        .map(|player| player as *mut PlayerIns)
        .map_err(|_| "the local player is not loaded".to_string())
}

fn read_souls(pgd: *mut PlayerGameData) -> Result<i32, String> {
    read_i32(pgd, HELD_SOULS_OFFSET)
}

fn write_souls(pgd: *mut PlayerGameData, value: i32) -> Result<(), String> {
    if pgd.is_null() {
        return Err("player data is missing".to_string());
    }
    unsafe { pgd.cast::<u8>().add(HELD_SOULS_OFFSET).cast::<i32>().write(value) };
    Ok(())
}

fn read_i32(pgd: *mut PlayerGameData, offset: usize) -> Result<i32, String> {
    if pgd.is_null() {
        return Err("player data is missing".to_string());
    }
    Ok(unsafe { pgd.cast::<u8>().add(offset).cast::<i32>().read() })
}

fn selected_quick_slot(pgd: *mut PlayerGameData) -> Result<usize, String> {
    let raw = read_i32(pgd, SELECTED_QUICK_SLOT_OFFSET)?;
    classify_slot(raw).map_err(|_| {
        let nearby = (0..16)
            .map(|index| unsafe {
                pgd.cast::<u8>()
                    .add(SELECTED_QUICK_SLOT_OFFSET.saturating_sub(4) + index)
                    .read()
            })
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" ");
        format!("selected quick slot at {SELECTED_QUICK_SLOT_OFFSET:#X} was {raw}; nearby bytes {nearby}")
    })
}

fn quick_item_id(pgd: *mut PlayerGameData, slot: usize) -> Result<i32, String> {
    read_i32(pgd, QUICK_ITEM_OFFSET + slot * 4)
}

fn item_quantity(pgd: *mut PlayerGameData, slot_value: i32) -> Option<u32> {
    let pgd = unsafe { pgd.as_mut()? };
    for entry in pgd.equipment.equip_inventory_data.items_data.items() {
        if item_matches(entry.item_id, slot_value) {
            return Some(entry.quantity);
        }
    }
    None
}

fn item_matches(item: darksouls3::sprj::ItemId, slot_value: i32) -> bool {
    let slot = slot_value as u32;
    let param = item.param_id();
    let combined = ((item.category() as u32) << 28) | param;
    slot == param || slot == combined
}

fn call_add_soul(pgd: *mut PlayerGameData, delta: i32) -> Result<(), String> {
    let address = *cached()?.add_soul.as_ref().map_err(|error| error.clone())?;
    let call: extern "C" fn(*mut PlayerGameData, i32) = unsafe { std::mem::transmute(address) };
    call(pgd, delta);
    Ok(())
}

fn cached() -> Result<&'static Located, String> {
    LOCATED.get().ok_or_else(|| "game patterns are not resolved yet".to_string())
}

fn game_man() -> Result<*mut u8, String> {
    let slot = *cached()?.game_man_slot.as_ref().map_err(|error| error.clone())?;
    let pointer = unsafe { (slot as *const usize).read() };
    if pointer < 0x10_000 {
        return Err("GameMan is null".to_string());
    }
    Ok(pointer as *mut u8)
}

fn locate_game_man_slot() -> Result<usize, String> {
    let site = find_unique(GAME_MAN_PATTERN)?;
    pattern::rip_target(site, 3, 7)
}

fn executable_address(addr: usize) -> bool {
    let Ok((image, base)) = pattern::module_slice() else {
        return false;
    };
    (base..base + image.len()).contains(&addr)
}

fn read_usize_checked(addr: usize) -> Result<usize, String> {
    if !readable_qword(addr) {
        return Err(format!("address {addr:#X} is not readable"));
    }
    Ok(unsafe { (addr as *const usize).read() })
}

fn copy_readable(addr: usize, dest: &mut [u8]) -> usize {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if dest.is_empty() || addr < 0x10_000 || addr > isize::MAX as usize - dest.len() {
        return 0;
    }
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let written = unsafe {
        VirtualQuery(
            Some(addr as *const _),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if written == 0 || info.State != MEM_COMMIT {
        return 0;
    }
    let protect = info.Protect.0 & 0xFF;
    let readable = matches!(protect, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80);
    let guarded = (info.Protect.0 & 0x100) != 0;
    if !readable || guarded {
        return 0;
    }
    let region_end = (info.BaseAddress as usize).saturating_add(info.RegionSize);
    let available = region_end.saturating_sub(addr);
    let count = dest.len().min(available);
    if count == 0 {
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(addr as *const u8, dest.as_mut_ptr(), count) };
    count
}

fn readable_qword(addr: usize) -> bool {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if addr < 0x10_000 || addr > isize::MAX as usize - 8 {
        return false;
    }
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let written = unsafe {
        VirtualQuery(
            Some(addr as *const _),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if written == 0 || info.State != MEM_COMMIT {
        return false;
    }
    let protect = info.Protect.0 & 0xFF;
    let readable = matches!(protect, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80);
    let guarded = (info.Protect.0 & 0x100) != 0;
    let region_end = (info.BaseAddress as usize).saturating_add(info.RegionSize);
    readable && !guarded && addr + 8 <= region_end
}

fn patterns_resolve() -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let bundle = darksouls3::rva::get();
        let program = Program::current();
        program.rva_to_va(bundle.game_data_man_ptr).is_ok()
            && program.rva_to_va(bundle.register_task).is_ok()
    }))
    .unwrap_or(false)
}

fn current_exe_is_ds3() -> bool {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    let mut buffer = [0u16; 520];
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    if length == 0 || length >= buffer.len() {
        return false;
    }
    let path = String::from_utf16_lossy(&buffer[..length]);
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .eq_ignore_ascii_case("DarkSoulsIII.exe")
}

fn create_npc_address() -> Result<usize, String> {
    let address = *cached()?.create_npc.as_ref().map_err(|error| error.clone())?;
    let mut bytes = [0u8; 7];
    if copy_readable(address, &mut bytes) != 7
        || bytes != [0x48, 0x83, 0xEC, bytes[3], 0xFF, 0xCA, 0x74]
    {
        return Err("the NPC spawn routine does not start with the expected prologue".to_string());
    }
    Ok(address)
}

fn debug_creator() -> Result<usize, String> {
    let world = unsafe { WorldChrMan::instance() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?;
    let slot = (world as *const WorldChrMan as usize).wrapping_add(DEBUG_CHR_CREATOR);
    let creator = read_usize_checked(slot)?;
    if creator < 0x10_000 {
        return Err("the debug character creator is not available".to_string());
    }
    Ok(creator)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct EnemyChoice {
    npc_id: u32,
    think_id: u32,
}

/// Boss models are c5000 to c5999.
pub fn is_boss_row(id: u32) -> bool {
    (500_000..600_000).contains(&id)
}

/// Only rows with a model and real health, leaving out the c8000 range and up, which holds
/// no regular enemies. Without bosses, very high health also rules out boss-like rows.
pub fn is_spawnable_enemy(id: u32, hp: i32, bosses: bool) -> bool {
    (100_000..800_000).contains(&id)
        && hp > 1
        && (bosses || (hp <= MAX_RANDOM_ENEMY_HP && !is_boss_row(id)))
}

/// Regular enemies share one team, so the most common team tells them from friendly NPCs.
/// Bosses may be on teams of their own, which `enemy_teams` adds.
pub fn most_common_team(teams: impl Iterator<Item = u8>) -> Option<u8> {
    let mut counts = [0u32; 256];
    for team in teams {
        counts[team as usize] += 1;
    }
    let (team, count) = counts.iter().enumerate().max_by_key(|(_, count)| **count)?;
    (*count > 0).then_some(team as u8)
}

pub fn enemy_teams(rows: &[(u32, u8)]) -> Vec<u8> {
    let regular = rows.iter().filter(|(id, _)| !is_boss_row(*id)).map(|(_, team)| *team);
    let mut teams: Vec<u8> = most_common_team(regular).into_iter().collect();
    teams.extend(rows.iter().filter(|(id, _)| is_boss_row(*id)).map(|(_, team)| *team));
    teams.sort_unstable();
    teams.dedup();
    teams
}

pub fn has_fight_logic(logic_id: i32, battle_goal_id: i32) -> bool {
    logic_id > 0 && battle_goal_id > 0 && battle_goal_id != GENERIC_BATTLE_GOAL
}

/// Prefers the AI rows placed enemies really run, because the table also holds idle and
/// scripted variants. Fight scripts only load with their own area, so an enemy from
/// elsewhere borrows a local AI. Its moves are written for another model, so it may act oddly.
pub fn ai_choices(npc_id: u32, think_ids: &[u32], placed: &[(u32, u32)], models: &[u32]) -> Vec<u32> {
    let model = npc_id / NPC_IDS_PER_MODEL;
    let own: Vec<u32> = placed
        .iter()
        .filter(|(placed_model, _)| *placed_model == model)
        .map(|(_, think)| *think)
        .collect();
    if !own.is_empty() {
        return own;
    }
    if models.contains(&model) {
        return think_candidates(npc_id, think_ids);
    }
    placed.iter().map(|(_, think)| *think).collect()
}

/// The AI rows an enemy can use. The NPC row itself holds 0, a generic AI that never
/// fights, because placed enemies get theirs from the map. AI rows are numbered like NPC
/// rows, so the same id is the best match, then any row for the same model.
pub fn think_candidates(npc_id: u32, think_ids: &[u32]) -> Vec<u32> {
    if think_ids.contains(&npc_id) {
        return vec![npc_id];
    }
    think_ids
        .iter()
        .copied()
        .filter(|id| id / NPC_IDS_PER_MODEL == npc_id / NPC_IDS_PER_MODEL)
        .collect()
}

/// "c1100_00" becomes 1100. The player model c0000 is not an enemy.
pub fn model_from_chr_name(name: &str) -> Option<u32> {
    let (model, _) = name.strip_prefix('c')?.split_once('_').unwrap_or((&name[1..], ""));
    let model = model.parse::<u32>().ok()?;
    (model != 0).then_some(model)
}

const BLOCK_CHR_SET_CAPACITY: usize = 0x80;
const BLOCK_CHR_SET_ENTRIES: usize = 0x88;
const CHR_SET_ENTRY_SIZE: usize = 0x38;
const MAX_CHRS_PER_BLOCK: u32 = 4096;
const CHR_DATA_NAME: usize = 0x130;
const CHR_DATA_HP_FIELD: usize = 0xD8;
/// Empty on spawns whose model never loaded. They stay invisible and do nothing.
const CHR_MODEL: usize = 0x48;
/// A spawn with no model by now is frozen.
const SPAWN_CHECK_FRAMES: u32 = 150;
/// While the list is full, the queue only looks again after this many frames.
const FULL_RECHECK_FRAMES: u32 = 15;
/// While the list is full, one living enemy is removed every this many checks.
const REPLACE_OLDEST_CHECKS: u32 = 4;
/// WorldChrMan keeps spawned characters here in creation order, oldest first.
const SPAWN_ORDER_BEGIN: usize = 0x2FF8;
/// The AI row a placed enemy really runs.
const ENEMY_THINK_ID: usize = 0x1FB8;
/// Inside the creator routine: the jump to the "delete last character" command.
const DELETE_LAST_JUMP: usize = 0x81;
/// Inside that command: the jump to the function that deletes any character.
const DELETE_CHR_JUMP: usize = 0x1F;
/// The goal of AI rows with no fight logic. Enemies that only follow map events use it.
const GENERIC_BATTLE_GOAL: i32 = 11000;

fn chr_name(chr: usize) -> Option<String> {
    let data = module_pointer(chr, DATA_MODULE_OFFSET).ok()?;
    let mut raw = [0u8; 16];
    if copy_readable(data + CHR_DATA_NAME, &mut raw) != raw.len() {
        return None;
    }
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16(&units).ok()
}

/// The model and AI row of each character in the loaded map. These models spawn reliably,
/// while other models are sometimes not available to the creator.
fn placed_enemies() -> Result<Vec<(u32, u32)>, String> {
    let world = unsafe { WorldChrMan::instance() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?;
    let slots = world.world_block_chr.as_ptr() as usize;
    let slot_size = std::mem::size_of_val(&world.world_block_chr) / world.world_block_chr.len();
    let mut placed = Vec::new();
    for block in (0..world.world_block_chr.len()).map(|index| slots + index * slot_size) {
        let capacity = read_u32_checked(block + BLOCK_CHR_SET_CAPACITY).unwrap_or(0);
        let entries = read_usize_checked(block + BLOCK_CHR_SET_ENTRIES).unwrap_or(0);
        if capacity == 0 || capacity > MAX_CHRS_PER_BLOCK || entries < 0x10_000 {
            continue;
        }
        for index in 0..capacity as usize {
            let chr = read_usize_checked(entries + index * CHR_SET_ENTRY_SIZE).unwrap_or(0);
            if chr < 0x10_000 {
                continue;
            }
            if let Some(model) = chr_name(chr).as_deref().and_then(model_from_chr_name) {
                placed.push((model, read_u32_checked(chr + ENEMY_THINK_ID).unwrap_or(0)));
            }
        }
    }
    placed.sort_unstable();
    placed.dedup();
    Ok(placed)
}

struct EnemyInFlight {
    previous: usize,
    frames: u32,
}

/// The creator makes one character at a time, so the rest wait here.
struct EnemyQueue {
    creator: usize,
    routine: usize,
    remaining: Vec<EnemyChoice>,
    pool: Vec<EnemyChoice>,
    replace_oldest: bool,
    limit: usize,
    full_frames: u32,
    pause_frames: u32,
    replacements: u32,
    checks: Vec<SpawnCheck>,
    in_flight: Option<EnemyInFlight>,
    started: u32,
}

static ENEMY_QUEUE: Mutex<Option<EnemyQueue>> = Mutex::new(None);

/// Spawned characters live in a small list of their own, and the creator silently fails
/// once it is full. Returns how many slots are used and how many there are.
fn spawned_chr_slots() -> Option<(usize, usize)> {
    let world = unsafe { WorldChrMan::instance() }.ok()?;
    let set = &world.debug_chr_set;
    Some((set.iter().count(), set.capacity as usize))
}

/// Enemies already being deleted do not count, but they keep their slot in the game's list.
fn spawned_chrs_full(limit: usize) -> bool {
    let Some((used, capacity)) = spawned_chr_slots() else {
        return false;
    };
    let requested = DELETE_REQUESTED.lock().unwrap_or_else(|poison| poison.into_inner());
    let active = spawned_chrs()
        .iter()
        .filter(|chr| !requested.contains(chr))
        .count();
    used >= capacity || active >= limit
}

fn spawned_chrs() -> Vec<usize> {
    let Ok(world) = (unsafe { WorldChrMan::instance() }) else {
        return Vec::new();
    };
    world
        .debug_chr_set
        .iter()
        .map(|entry| &*entry.chr as *const _ as usize)
        .collect()
}

fn chr_health(chr: usize) -> Option<i32> {
    let data = module_pointer(chr, DATA_MODULE_OFFSET).ok()?;
    read_u32_checked(data + CHR_DATA_HP_FIELD).ok().map(|hp| hp as i32)
}

fn dead_spawns() -> Vec<usize> {
    spawned_chrs()
        .into_iter()
        .filter(|chr| chr_health(*chr).is_some_and(|hp| hp <= 0))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum SpawnState {
    Fine,
    Gone,
    Frozen,
}

pub fn spawn_state(present: bool, model: usize) -> SpawnState {
    match (present, model) {
        (false, _) => SpawnState::Gone,
        (true, 0) => SpawnState::Frozen,
        (true, _) => SpawnState::Fine,
    }
}

struct SpawnCheck {
    chr: usize,
    frames: u32,
}

fn poll_spawn_checks(queue: &mut EnemyQueue) {
    if queue.checks.is_empty() {
        return;
    }
    let present = spawned_chrs();
    let mut frozen = Vec::new();
    let mut failed = 0;
    for check in queue.checks.iter_mut() {
        check.frames += 1;
        let exists = present.contains(&check.chr);
        if exists && check.frames < SPAWN_CHECK_FRAMES {
            continue;
        }
        check.frames = SPAWN_CHECK_FRAMES;
        let model = if exists {
            read_usize_checked(check.chr + CHR_MODEL).unwrap_or(0)
        } else {
            0
        };
        let state = spawn_state(exists, model);
        match state {
            SpawnState::Fine => {}
            SpawnState::Gone => failed += 1,
            SpawnState::Frozen => {
                failed += 1;
                frozen.push(check.chr);
            }
        }
    }
    queue.checks.retain(|check| check.frames < SPAWN_CHECK_FRAMES);
    if !frozen.is_empty() {
        let mut requested = DELETE_REQUESTED.lock().unwrap_or_else(|poison| poison.into_inner());
        if let Err(error) = delete_spawns(&frozen, &mut requested) {
            crate::log::log(format!("spawn enemy: frozen enemies not removed: {error}"));
        }
    }
    for _ in 0..failed {
        replace_failed_spawn(queue);
    }
}

fn replace_failed_spawn(queue: &mut EnemyQueue) {
    if queue.replacements < MAX_SPAWN_REPLACEMENTS {
        queue.replacements += 1;
        let replacement = queue.pool[random_index(queue.pool.len())];
        queue.remaining.push(replacement);
    }
}

fn jump_target(address: usize) -> Result<usize, String> {
    let mut bytes = [0u8; 5];
    if copy_readable(address, &mut bytes) != bytes.len() || bytes[0] != 0xE9 {
        return Err(format!("expected a jump at {address:#x}"));
    }
    let offset = i32::from_ne_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    Ok((address + bytes.len()).wrapping_add_signed(offset as isize))
}

/// The game's own function for deleting a character, found through the creator routine.
fn delete_chr_address() -> Result<usize, String> {
    let delete_last = jump_target(create_npc_address()? + DELETE_LAST_JUMP)?;
    let delete = jump_target(delete_last + DELETE_CHR_JUMP)?;
    let mut bytes = [0u8; 5];
    if copy_readable(delete, &mut bytes) != bytes.len() || bytes != [0x48, 0x85, 0xD2, 0x0F, 0x84] {
        return Err("the character delete function does not look as expected".to_string());
    }
    Ok(delete)
}

/// Deletion finishes a few frames later, so each character is only requested once.
static DELETE_REQUESTED: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn delete_spawns(chrs: &[usize], requested: &mut Vec<usize>) -> Result<(), String> {
    let delete = delete_chr_address()?;
    let world = unsafe { WorldChrMan::instance() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?;
    let world = world as *const WorldChrMan as usize;
    let call: extern "C" fn(usize, usize) = unsafe { std::mem::transmute(delete) };
    for chr in chrs.iter().copied() {
        if !requested.contains(&chr) {
            call(world, chr);
            requested.push(chr);
        }
    }
    Ok(())
}

/// Makes room in a full list: dead enemies go first, then the oldest living one if allowed.
/// A delete can take a while to finish, so the next one does not wait for it.
fn free_spawn_slot(replace_oldest: bool, full_frames: u32) -> Result<(), String> {
    let mut requested = DELETE_REQUESTED.lock().unwrap_or_else(|poison| poison.into_inner());
    let present = spawned_chrs();
    requested.retain(|chr| present.contains(chr));
    let dead = dead_spawns();
    if !dead.is_empty() {
        delete_spawns(&dead, &mut requested)?;
    }
    if !replace_oldest || full_frames % REPLACE_OLDEST_CHECKS != 0 {
        return Ok(());
    }
    let world = unsafe { WorldChrMan::instance() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?;
    let order = world as *const WorldChrMan as usize + SPAWN_ORDER_BEGIN;
    let (begin, end) = (read_usize_checked(order)?, read_usize_checked(order + 8)?);
    if begin == end {
        return Ok(());
    }
    let oldest = read_usize_checked(begin)?;
    if present.contains(&oldest) {
        delete_spawns(&[oldest], &mut requested)?;
    }
    Ok(())
}

pub fn spawn_random_enemy(
    amount: i32,
    whole_game: bool,
    bosses: bool,
    only_bosses: bool,
    replace_oldest: bool,
    limit: usize,
) -> Result<String, String> {
    let bosses = bosses || only_bosses;
    let routine = create_npc_address()?;
    let creator = debug_creator()?;
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the NPC list is unavailable: {error:?}"))?;
    let placed = placed_enemies()?;
    let mut models: Vec<u32> = placed.iter().map(|(model, _)| *model).collect();
    models.dedup();
    let npcs = regulation.get_param::<NPC_PARAM_ST>();
    let thinks = regulation.get_param::<NPC_THINK_PARAM_ST>();
    let mut think_ids: Vec<u32> = thinks
        .iter()
        .filter(|(id, row)| *id > 0 && has_fight_logic(row.logic_id(), row.battle_goal_id()))
        .map(|(id, _)| id as u32)
        .collect();
    think_ids.sort_unstable();
    let placed: Vec<(u32, u32)> = placed
        .into_iter()
        .filter(|(_, think)| think_ids.binary_search(think).is_ok())
        .collect();
    let team_rows: Vec<(u32, u8)> = npcs
        .iter()
        .filter(|(id, row)| is_spawnable_enemy(*id as u32, row.hp(), bosses))
        .map(|(id, row)| (id as u32, row.team_type()))
        .collect();
    let teams = enemy_teams(&team_rows);
    let pool: Vec<EnemyChoice> = npcs
        .iter()
        .filter(|(id, row)| {
            is_spawnable_enemy(*id as u32, row.hp(), bosses)
                && (!only_bosses || is_boss_row(*id as u32))
                && teams.contains(&row.team_type())
                && (whole_game || models.binary_search(&(*id as u32 / NPC_IDS_PER_MODEL)).is_ok())
        })
        .filter_map(|(id, _)| {
            let npc_id = id as u32;
            let candidates = ai_choices(npc_id, &think_ids, &placed, &models);
            if candidates.is_empty() {
                return None;
            }
            Some(EnemyChoice {
                npc_id,
                think_id: candidates[random_index(candidates.len())],
            })
        })
        .collect();
    if pool.is_empty() {
        return Err("no enemies that can be spawned were found".to_string());
    }
    let picks: Vec<EnemyChoice> = (0..amount.max(1))
        .map(|_| pool[random_index(pool.len())])
        .collect();
    let ids: Vec<u32> = picks.iter().map(|pick| pick.npc_id).collect();
    let mut slot = ENEMY_QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
    let found = pool.len();
    match slot.as_mut() {
        Some(queue) => {
            queue.remaining.extend(picks);
            queue.pool = pool;
            queue.replace_oldest = replace_oldest;
            queue.limit = limit;
            queue.replacements = 0;
        }
        None => {
            *slot = Some(EnemyQueue {
                creator,
                routine,
                remaining: picks,
                pool,
                replace_oldest,
                limit,
                full_frames: 0,
                pause_frames: 0,
                replacements: 0,
                checks: Vec::new(),
                in_flight: None,
                started: 0,
            });
        }
    }
    Ok(format!("spawning enemies {ids:?}, picked from {found}"))
}

fn start_enemy_spawn(queue: &mut EnemyQueue, enemy: EnemyChoice) -> Result<(), String> {
    let creator = queue.creator;
    let distance = SPAWN_DISTANCE_METERS + (queue.started % SPAWN_DISTANCE_STEPS) as f32;
    write_u32_checked(creator + CREATOR_CHR_ID, enemy.npc_id / NPC_IDS_PER_MODEL)?;
    write_bytes_checked(creator + CREATOR_DISTANCE, &distance.to_ne_bytes())?;
    write_bytes_checked(creator + CREATOR_HEIGHT, &SPAWN_HEIGHT_METERS.to_ne_bytes())?;
    write_bytes_checked(creator + CREATOR_ANGLE, &0f32.to_ne_bytes())?;
    write_u32_checked(creator + CREATOR_NPC_PARAM_ID, enemy.npc_id)?;
    write_u32_checked(creator + CREATOR_THINK_ID, enemy.think_id)?;
    let previous = read_usize_checked(creator + CREATOR_RESULT).unwrap_or(0);
    let call: extern "C" fn(usize, u32) = unsafe { std::mem::transmute(queue.routine) };
    call(creator, CREATE_NPC_COMMAND);
    queue.started += 1;
    queue.in_flight = Some(EnemyInFlight {
        previous,
        frames: 0,
    });
    Ok(())
}

pub fn poll_enemy_queue() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = ENEMY_QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(queue) = slot.as_mut() else {
            return;
        };
        // The world goes away before the load screen shows, and the creator needs it.
        if load_screen_open() || unsafe { WorldChrMan::instance() }.is_err() {
            slot.take();
            return;
        }
        poll_spawn_checks(queue);
        if let Some(flight) = queue.in_flight.as_mut() {
            flight.frames += 1;
            let chr = read_usize_checked(queue.creator + CREATOR_RESULT).unwrap_or(0);
            if chr >= 0x10_000 && chr != flight.previous {
                queue.checks.push(SpawnCheck {
                    chr,
                    frames: 0,
                });
                queue.in_flight = None;
            } else if flight.frames >= SPAWN_WAIT_FRAMES {
                queue.in_flight = None;
                replace_failed_spawn(queue);
            }
            return;
        }
        if queue.pause_frames > 0 {
            queue.pause_frames -= 1;
            return;
        }
        let Some(enemy) = queue.remaining.pop() else {
            if queue.checks.is_empty() {
                slot.take();
            }
            return;
        };
        if spawned_chrs_full(queue.limit) {
            if let Err(error) = free_spawn_slot(queue.replace_oldest, queue.full_frames) {
                crate::log::log(format!("enemy spawns stopped: {error}"));
                slot.take();
                return;
            }
            queue.full_frames += 1;
            queue.pause_frames = FULL_RECHECK_FRAMES;
            queue.remaining.push(enemy);
            return;
        }
        queue.full_frames = 0;
        let mut ready = [0u8; 1];
        if copy_readable(queue.creator + CREATOR_READY, &mut ready) != 1 || ready[0] == 0 {
            queue.remaining.push(enemy);
            return;
        }
        if let Err(error) = start_enemy_spawn(queue, enemy) {
            crate::log::log(format!("spawn enemy: {} failed: {error}", enemy.npc_id));
        }
    }));
}

pub fn schemas_json() -> Value {
    json!([
        {
            "id": "adjust_souls",
            "name": "Adjust souls",
            "description": "Adds a signed amount to held souls. Negative values remove souls and the result is clamped at 0.",
            "arguments": [
                { "name": "delta", "type": "int", "default": -100, "min": -999999999, "max": 999999999 },
                { "name": "scaleWithLevel", "type": "bool", "default": 0, "min": 0, "max": 1, "label": "Scale with level", "hint": "Grows with the cost of your next level. The amount is what you lose at level 10, so -100 takes about 1800 souls at level 50 and 7400 at level 100." }
            ]
        },
        {
            "id": "use_selected_item",
            "name": "Use selected item",
            "description": "Uses whatever item is in the current quick-item slot.",
            "arguments": []
        },
        {
            "id": "kill_player",
            "name": "Kill player",
            "description": "Sets the local player's HP to 0 so the game performs its normal death and respawn flow.",
            "arguments": []
        },
        {
            "id": "fake_death",
            "name": "Fake Death",
            "description": "Plays the death animation, grey screen and YOU DIED banner, fades to black, then stands you back up at your previous HP and souls.",
            "arguments": []
        },
        {
            "id": "show_you_died",
            "name": "Show YOU DIED",
            "description": "Shows the YOU DIED banner with its sound. The player stays alive and the area does not reload.",
            "arguments": []
        },
        {
            "id": "empty_stamina",
            "name": "Empty Stamina",
            "description": "Sets current stamina to 0. Being on a ladder with no stamina makes you fall off.",
            "arguments": []
        },
        {
            "id": "no_stamina",
            "name": "No Stamina",
            "description": "Holds stamina empty for the chosen number of seconds, blocking sprinting, rolling and attacks.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 5, "min": 1, "max": 60 }]
        },
        {
            "id": "decrease_speed",
            "name": "Decrease Speed",
            "description": "Slows the player by a percentage for the chosen time. Adds together with Increase Speed.",
            "arguments": [
                { "name": "percent", "type": "int", "default": 30, "min": 1, "max": 90 },
                { "name": "seconds", "type": "int", "default": 30, "min": 1, "max": 300 },
                { "name": "movementOnly", "type": "bool", "label": "Movement only (experimental)", "hint": "Only walking, running and rolling change speed. Attacks can still start at the changed speed, because the game reports them a moment late.", "default": 0, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "increase_speed",
            "name": "Increase Speed",
            "description": "Speeds up the player by a percentage for the chosen time. Adds together with Decrease Speed.",
            "arguments": [
                { "name": "percent", "type": "int", "default": 30, "min": 1, "max": 300 },
                { "name": "seconds", "type": "int", "default": 30, "min": 1, "max": 300 },
                { "name": "movementOnly", "type": "bool", "label": "Movement only (experimental)", "hint": "Only walking, running and rolling change speed. Attacks can still start at the changed speed, because the game reports them a moment late.", "default": 0, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "one_hp",
            "name": "1 HP",
            "description": "Sets current HP to 1.",
            "arguments": []
        },
        {
            "id": "one_hit_run",
            "name": "1 Hit Run",
            "description": "Holds HP at 1 for the chosen number of seconds, so healing does nothing and any hit kills.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 30, "min": 1, "max": 300 }]
        },
        {
            "id": "invincible",
            "name": "Invincible",
            "description": "The player takes no hit damage for the chosen number of seconds. A second use keeps the longer remaining time.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 30, "min": 1, "max": 300 }]
        },
        {
            "id": "random_weapon",
            "name": "Random Weapon",
            "description": "Gives you a random weapon and equips it in right hand slot 1.",
            "arguments": [
                { "name": "weaponLevel", "type": "choice", "label": "Weapon level", "options": ["+0", "Random", "Same as right hand 1"], "default": 0, "min": 0, "max": 2 },
                { "name": "infusion", "type": "choice", "label": "Infusion", "options": ["None", "Random", "Same as right hand 1"], "default": 0, "min": 0, "max": 2 },
                { "name": "removeOld", "type": "bool", "label": "Remove previous weapon", "hint": "Permanently deletes the weapon that was in right hand 1, including its upgrades. Fists are never removed.", "default": 1, "min": 0, "max": 1 },
                { "name": "includeMissingDlc", "type": "bool", "label": "Include weapons from DLC you do not have (experimental)", "hint": "Worked in a short test without the DLC installed, but not every weapon has been tried.", "default": 0, "min": 0, "max": 1 },
                { "name": "usableOnly", "type": "bool", "label": "Usable two-handed only", "hint": "Skips weapons your stats cannot use, counting Strength at 1.5x for two-handing.", "default": 1, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "random_armor",
            "name": "Random Armor",
            "description": "Gives you a random piece for each armor slot and equips all four.",
            "arguments": [
                { "name": "armorStyle", "type": "choice", "label": "Armor style", "hint": "Matching set picks one armor set. If it has no piece for a slot, that slot stays as it is.", "options": ["Random pieces", "Matching set", "Either at random"], "default": 0, "min": 0, "max": 2 },
                { "name": "removeOld", "type": "bool", "label": "Remove previous armor", "hint": "Permanently deletes the armor you were wearing. Empty slots are skipped.", "default": 1, "min": 0, "max": 1 },
                { "name": "includeMissingDlc", "type": "bool", "label": "Include armor from DLC you do not have (experimental)", "hint": "Weapons from a missing DLC worked in a short test, but armor has not been tried.", "default": 0, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "temporary_armor",
            "name": "Temporary Armor",
            "description": "Swaps all armor slots for a random outfit, then puts your own armor back when the time is up.",
            "arguments": [
                { "name": "seconds", "type": "int", "default": 60, "min": 1, "max": 600 },
                { "name": "armorStyle", "type": "choice", "label": "Armor style", "hint": "Matching set picks one armor set. If it has no piece for a slot, that slot stays as it is.", "options": ["Random pieces", "Matching set", "Either at random"], "default": 0, "min": 0, "max": 2 },
                { "name": "includeMissingDlc", "type": "bool", "label": "Include armor from DLC you do not have (experimental)", "hint": "Weapons from a missing DLC worked in a short test, but armor has not been tried.", "default": 0, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "bombardier",
            "name": "Bombardier",
            "description": "Takes away all your weapons, dresses you in the Mirrah Chain Set and gives you endless Black Firebombs in a free quick slot, then puts everything back when the time is up.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 60, "min": 5, "max": 600 }]
        },
        {
            "id": "shuffle_stats",
            "name": "Shuffle Stats",
            "description": "Randomly swaps your nine stats with each other, so your level stays the same.",
            "arguments": [
                { "name": "seconds", "type": "int", "label": "Seconds", "hint": "How long until your real stats come back. 0 keeps the shuffle, and it saves to your character.", "default": 0, "min": 0, "max": 600 }
            ]
        },
        {
            "id": "oversaturate",
            "name": "Oversaturate",
            "description": "Pushes the colours far past normal for the chosen number of seconds, for a badly over-edited look.",
            "arguments": [
                { "name": "seconds", "type": "int", "default": 15, "min": 1, "max": 300 },
                { "name": "percent", "type": "int", "default": 2000, "min": 100, "max": 2000 }
            ]
        },
        {
            "id": "blind",
            "name": "Blind",
            "description": "Fades the screen to black, holds it for the chosen number of seconds, then fades back in.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 5, "min": 1, "max": 60 }]
        },
        {
            "id": "freeze",
            "name": "Freeze",
            "description": "Freezes the player in place for the chosen number of seconds. The camera still moves.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 5, "min": 1, "max": 60 }]
        },
        {
            "id": "invert_controls",
            "name": "Invert Controls",
            "description": "Inverts the controller sticks for the chosen number of seconds. Controller only.",
            "arguments": [
                { "name": "seconds", "type": "int", "default": 15, "min": 1, "max": 300 },
                { "name": "camera", "type": "bool", "label": "Invert camera too", "hint": "Also flips the right stick. Off means only movement is inverted.", "default": 1, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "set_health_percent",
            "name": "Set health percent",
            "description": "Sets current HP to a percentage of the local player's maximum HP.",
            "arguments": [{ "name": "percent", "type": "int", "default": 50, "min": 1, "max": 100 }]
        },
        {
            "id": "set_health_random_percent",
            "name": "Set health to random percent",
            "description": "Sets current HP to a random value from 1% through 100% of maximum HP.",
            "arguments": []
        },
        {
            "id": "spawn_random_enemy",
            "name": "Spawn Random Enemy",
            "description": "Spawns random enemies near you, from the current map unless Whole game is on.",
            "arguments": [
                { "name": "amount", "type": "int", "default": 1, "min": 1, "max": 10 },
                { "name": "limit", "type": "int", "label": "Enemy limit", "hint": "How many spawned enemies can be active at once. The game itself stops at 16.", "default": 5, "min": 1, "max": 16 },
                { "name": "wholeGame", "type": "bool", "label": "Whole game (experimental)", "hint": "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly.", "default": 0, "min": 0, "max": 1 },
                { "name": "bosses", "type": "bool", "label": "Include bosses", "hint": "Bosses can spawn too. Many rely on map events, so some may act oddly.", "default": 1, "min": 0, "max": 1 },
                { "name": "onlyBosses", "type": "bool", "label": "Only bosses", "hint": "Spawns nothing but bosses. Without Whole game, only bosses from the current map can appear.", "default": 0, "min": 0, "max": 1 },
                { "name": "replaceOldest", "type": "bool", "label": "Replace oldest at limit", "hint": "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot.", "default": 1, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "dump_player_data",
            "name": "Dump player data",
            "description": "Writes the local player data block to a file, including the reported soul count.",
            "arguments": [{ "name": "soulCount", "type": "int", "default": 0, "min": 0, "max": 999999999 }]
        },
        {
            "id": "watch_item_button",
            "name": "Watch item button",
            "description": "Reads the player's controller memory for about 10 seconds. Press and release the use-item button once during that time. The game is not changed.",
            "arguments": []
        }
    ])
}

const WATCH_FRAMES: usize = 600;
const REGION_LEN: usize = 0x200;
const HEAD_LEN: usize = 64;

struct Track {
    ready: bool,
    first: u8,
    current: u8,
    changes: u16,
    values: [u8; 8],
    value_len: u8,
}

impl Track {
    fn observe(&mut self, value: u8) {
        if !self.ready {
            self.ready = true;
            self.first = value;
            self.current = value;
            self.values[0] = value;
            self.value_len = 1;
            return;
        }
        if value == self.current {
            return;
        }
        self.changes = self.changes.saturating_add(1);
        if (self.value_len as usize) < self.values.len() {
            self.values[self.value_len as usize] = value;
            self.value_len += 1;
        }
        self.current = value;
    }
}

struct RegionWatch {
    label: &'static str,
    address: usize,
    vtable_in_exe: bool,
    seen: u16,
    bytes: Vec<Track>,
    bits: Vec<Track>,
    head: [u8; HEAD_LEN],
}

struct ButtonWatch {
    remaining: usize,
    regions: Vec<RegionWatch>,
}

static BUTTON_WATCH: Mutex<Option<ButtonWatch>> = Mutex::new(None);

pub fn start_button_watch() -> Result<String, String> {
    let low_level = crate::input::start_trace()?;
    let player = local_player()? as usize;
    let regions = watch_regions(player);
    if regions.is_empty() {
        return Err("the player has no readable controller objects".to_string());
    }
    let mut slot = BUTTON_WATCH.lock().unwrap_or_else(|poison| poison.into_inner());
    if slot.is_some() {
        return Err("a button watch is already running".to_string());
    }
    let labels = regions
        .iter()
        .map(|region| format!("{}={:#X}", region.label, region.address))
        .collect::<Vec<_>>()
        .join(" ");
    *slot = Some(ButtonWatch {
        remaining: WATCH_FRAMES,
        regions,
    });
    crate::log::log(format!("button watch started {labels}"));
    Ok(format!(
        "{low_level} The existing player-object watch is running at the same time; leave other controls still."
    ))
}

pub fn poll_button_watch() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let finished = {
            let mut slot = BUTTON_WATCH.lock().unwrap_or_else(|poison| poison.into_inner());
            let done = match slot.as_mut() {
                Some(watch) => {
                    for region in &mut watch.regions {
                        sample_region(region);
                    }
                    watch.remaining = watch.remaining.saturating_sub(1);
                    watch.remaining == 0
                }
                None => false,
            };
            if done { slot.take() } else { None }
        };
        if let Some(watch) = finished {
            finish_button_watch(watch);
        }
    }));
}

fn watch_regions(player: usize) -> Vec<RegionWatch> {
    let modules = read_usize_checked(player.wrapping_add(0x1F90))
        .ok()
        .filter(|pointer| *pointer >= 0x10_000);
    let found = [
        ("player_ctrl", deref_pointer(player, 0x50)),
        ("pad", deref_pointer(player, 0x58)),
        ("action_flag", modules.and_then(|base| deref_pointer(base, 0x00))),
        ("action_request", modules.and_then(|base| deref_pointer(base, 0x80))),
    ];
    found
        .into_iter()
        .filter_map(|(label, address)| address.map(|address| new_region(label, address)))
        .collect()
}

fn new_region(label: &'static str, address: usize) -> RegionWatch {
    let vtable_in_exe = read_usize_checked(address)
        .map(executable_address)
        .unwrap_or(false);
    RegionWatch {
        label,
        address,
        vtable_in_exe,
        seen: 0,
        bytes: (0..REGION_LEN).map(|_| Track::default()).collect(),
        bits: (0..REGION_LEN * 8).map(|_| Track::default()).collect(),
        head: [0; HEAD_LEN],
    }
}

impl Default for Track {
    fn default() -> Self {
        Self {
            ready: false,
            first: 0,
            current: 0,
            changes: 0,
            values: [0; 8],
            value_len: 0,
        }
    }
}

fn deref_pointer(base: usize, offset: usize) -> Option<usize> {
    let pointer = read_usize_checked(base.checked_add(offset)?).ok()?;
    (pointer >= 0x10_000).then_some(pointer)
}

fn sample_region(region: &mut RegionWatch) {
    let mut buffer = [0u8; REGION_LEN];
    let copied = copy_readable(region.address, &mut buffer);
    if copied == 0 {
        return;
    }
    if region.seen == 0 {
        let head = copied.min(HEAD_LEN);
        region.head[..head].copy_from_slice(&buffer[..head]);
    }
    region.seen = region.seen.saturating_add(1);
    for index in 0..copied {
        region.bytes[index].observe(buffer[index]);
        for bit in 0..8 {
            region.bits[index * 8 + bit].observe((buffer[index] >> bit) & 1);
        }
    }
}

fn finish_button_watch(watch: ButtonWatch) {
    let report = render_watch(&watch);
    let pulse_lines = report.lines().filter(|line| line.contains(" pulse ")).count();
    match write_watch_report(&report) {
        Ok(path) => crate::log::log(format!(
            "button watch wrote {} pulses={pulse_lines}",
            path.display()
        )),
        Err(error) => crate::log::log(format!("button watch could not write the report: {error}")),
    }
}

fn write_watch_report(report: &str) -> Result<std::path::PathBuf, String> {
    let dir = crate::log::app_dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("button-watch-{stamp}.txt"));
    std::fs::write(&path, report).map_err(|error| error.to_string())?;
    Ok(path)
}

fn render_watch(watch: &ButtonWatch) -> String {
    let mut text = format!("frames={WATCH_FRAMES}\nregions={}\n", watch.regions.len());
    for region in &watch.regions {
        text.push_str(&format!(
            "\nregion {} addr={:#X} seen={} vtable={}\nhead=",
            region.label,
            region.address,
            region.seen,
            if region.vtable_in_exe { "yes" } else { "no" }
        ));
        for byte in &region.head {
            text.push_str(&format!("{byte:02X}"));
        }
        text.push('\n');
        let mut listed = 0usize;
        let mut total = 0usize;
        for (offset, track) in region.bytes.iter().enumerate() {
            if let Some(kind) = change_kind(track.changes, track.first, track.current) {
                total += 1;
                if listed >= 80 {
                    continue;
                }
                listed += 1;
                text.push_str(&format!(
                    "byte +{offset:#06X} {kind} changes={} {}\n",
                    track.changes,
                    format_values(track)
                ));
            }
        }
        for offset in 0..REGION_LEN {
            let byte_kind = change_kind(
                region.bytes[offset].changes,
                region.bytes[offset].first,
                region.bytes[offset].current,
            );
            if byte_kind == Some("pulse") {
                continue;
            }
            for bit in 0..8 {
                let track = &region.bits[offset * 8 + bit];
                if change_kind(track.changes, track.first, track.current) != Some("pulse") {
                    continue;
                }
                total += 1;
                if listed >= 80 {
                    continue;
                }
                listed += 1;
                text.push_str(&format!(
                    "bit +{offset:#06X}.{bit} pulse changes={}\n",
                    track.changes
                ));
            }
        }
        let noisy = region
            .bytes
            .iter()
            .filter(|track| track.changes > 8)
            .count();
        text.push_str(&format!(
            "noisyBytes={noisy} listed={listed} hidden={}\n",
            total.saturating_sub(listed)
        ));
    }
    text
}

fn change_kind(changes: u16, first: u8, last: u8) -> Option<&'static str> {
    if changes == 0 {
        return None;
    }
    if changes <= 8 && last == first {
        Some("pulse")
    } else if changes <= 8 {
        Some("edge")
    } else {
        None
    }
}

fn format_values(track: &Track) -> String {
    track.values[..track.value_len as usize]
        .iter()
        .map(|value| format!("{value:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::{
        apply_soul_delta, bombs_to_remove, can_restore_stats, is_estus, next_level_cost, pick_bomb_slot,
        shuffled_stats,
        ArmorSwap, Bombardier, PendingRestores, RemovedWeapon, ShuffledStats, TemporaryArmor, soul_delta_for_level, armor_available, armor_sets, can_restore_armor, classify_slot, ArmorStyle, combined_speed, is_real_armor, health_at_percent, is_base_weapon_id,
        is_empty_item, is_listed_armor, is_movement_animation, is_removable_weapon, pick_infusion,
        pick_level,
        ai_choices, enemy_teams, has_fight_logic, is_spawnable_enemy, most_common_team, spawn_state,
        SpawnState, model_from_chr_name, think_candidates, usable_two_handed, wait_reason, weapon_available,
        weapon_infusion, weapon_level, WeaponTrait, with_no_damage_bit,
    };

    #[test]
    fn no_damage_bit_leaves_the_other_flags_alone() {
        assert_eq!(with_no_damage_bit(0, true), 0x02);
        assert_eq!(with_no_damage_bit(0x05, true), 0x07);
        assert_eq!(with_no_damage_bit(0x07, false), 0x05);
        assert_eq!(with_no_damage_bit(0xFF, false), 0xFD);
    }

    #[test]
    fn dlc_weapons_need_their_dlc() {
        let (crow_talons, spear, longsword) = (11_050_000, 9_240_000, 2_000_000);
        assert!(weapon_available(longsword, false, false));
        assert!(!weapon_available(crow_talons, false, true));
        assert!(weapon_available(crow_talons, true, false));
        assert!(!weapon_available(spear, true, false));
        assert!(weapon_available(spear, false, true));
    }

    #[test]
    fn armor_rules_match_the_dlc_and_empty_slots() {
        let (vilhelm_helm, ringed_knight_hood, plain) = (20_000_000, 63_500_000, 1_000_000);
        assert!(!is_real_armor(900_000));
        assert!(is_real_armor(plain));
        assert!(armor_available(plain, false, false));
        assert!(!armor_available(vilhelm_helm, false, true));
        assert!(armor_available(vilhelm_helm, true, false));
        assert!(!armor_available(ringed_knight_hood, true, false));
        assert!(armor_available(ringed_knight_hood, false, true));
    }

    #[test]
    fn armor_groups_into_sets_and_skips_lone_pieces() {
        let pools = [
            vec![20_000_000, 83_500_000],
            vec![20_001_000, 64_501_000],
            vec![20_002_000, 64_502_000],
            vec![20_003_000, 64_503_000],
        ];
        let sets = armor_sets(&pools);
        assert_eq!(sets.len(), 2);
        assert!(sets.contains(&[
            Some(20_000_000),
            Some(20_001_000),
            Some(20_002_000),
            Some(20_003_000),
        ]));
        assert!(sets.contains(&[None, Some(64_501_000), Some(64_502_000), Some(64_503_000)]));
    }

    #[test]
    fn armor_style_decides_when_a_set_is_used() {
        assert!(!ArmorStyle::from_index(0).uses_set(1));
        assert!(ArmorStyle::from_index(1).uses_set(0));
        assert!(!ArmorStyle::from_index(2).uses_set(0));
        assert!(ArmorStyle::from_index(2).uses_set(1));
    }

    #[test]
    fn armor_only_goes_back_when_it_is_safe() {
        assert!(can_restore_armor(false, true, true, false));
        assert!(!can_restore_armor(true, true, true, false));
        assert!(!can_restore_armor(false, false, true, false));
        assert!(!can_restore_armor(false, true, false, false));
        assert!(!can_restore_armor(false, true, true, true));
    }

    #[test]
    fn effects_wait_for_a_safe_moment() {
        assert_eq!(wait_reason(false, true, true, false), None);
        assert!(wait_reason(true, true, true, false).is_some());
        assert!(wait_reason(false, false, true, false).is_some());
        assert!(wait_reason(false, true, false, false).is_some());
        assert!(wait_reason(false, true, true, true).is_some());
    }

    #[test]
    fn chr_names_give_their_model() {
        assert_eq!(model_from_chr_name("c1100_00"), Some(1100));
        assert_eq!(model_from_chr_name("c1100"), Some(1100));
        assert_eq!(model_from_chr_name("c0000_00"), None);
        assert_eq!(model_from_chr_name("m1100"), None);
        assert_eq!(model_from_chr_name(""), None);
    }

    #[test]
    fn only_real_non_boss_enemies_can_spawn() {
        assert!(is_spawnable_enemy(110_000, 300, false));
        assert!(!is_spawnable_enemy(27_700, 300, true));
        assert!(!is_spawnable_enemy(110_000, 0, true));
        assert!(!is_spawnable_enemy(110_000, 1, true));
        assert!(!is_spawnable_enemy(890_230, 300, true));
        assert!(!is_spawnable_enemy(110_000, 20_000, false));
        assert!(!is_spawnable_enemy(528_000, 300, false));
        assert!(is_spawnable_enemy(528_000, 20_000, true));
    }

    #[test]
    fn generic_ai_rows_do_not_count_as_fighters() {
        assert!(has_fight_logic(10_000, 224_000));
        assert!(!has_fight_logic(10_000, 11_000));
        assert!(!has_fight_logic(0, 224_000));
        assert!(!has_fight_logic(10_000, 0));
    }

    #[test]
    fn spawns_without_a_model_count_as_frozen() {
        assert_eq!(spawn_state(false, 0), SpawnState::Gone);
        assert_eq!(spawn_state(true, 0), SpawnState::Frozen);
        assert_eq!(spawn_state(true, 0x7FF4_9F0A_6F00), SpawnState::Fine);
    }

    #[test]
    fn boss_teams_count_as_enemy_teams() {
        let rows = [(110_000, 6), (120_000, 6), (620_000, 24), (528_000, 7)];
        assert_eq!(enemy_teams(&rows), vec![6, 7]);
    }

    #[test]
    fn the_enemy_team_is_the_most_common_one() {
        assert_eq!(most_common_team([6, 6, 24].into_iter()), Some(6));
        assert_eq!(most_common_team(std::iter::empty()), None);
    }

    #[test]
    fn enemies_use_what_placed_enemies_really_run() {
        let rows = [110_000, 110_054, 120_000, 130_000];
        let placed = [(1100, 110_054), (1200, 120_000)];
        let models = [1100, 1200, 1300];
        assert_eq!(ai_choices(110_070, &rows, &placed, &models), vec![110_054]);
        assert_eq!(ai_choices(130_000, &rows, &placed, &models), vec![130_000]);
        assert_eq!(ai_choices(140_000, &rows, &placed, &models), vec![110_054, 120_000]);
        assert_eq!(ai_choices(140_000, &rows, &[], &models), Vec::<u32>::new());
    }

    #[test]
    fn enemies_prefer_their_own_ai_row_then_their_model() {
        let rows = [110_000, 110_010, 120_000];
        assert_eq!(think_candidates(110_010, &rows), vec![110_010]);
        assert_eq!(think_candidates(110_055, &rows), vec![110_000, 110_010]);
        assert_eq!(think_candidates(130_000, &rows), Vec::<u32>::new());
    }

    #[test]
    fn unused_armor_rows_are_not_listed() {
        for id in [1_000_000, 1_001_000, 83_501_000, 900_000] {
            assert!(!is_listed_armor(id), "{id}");
        }
        for id in [20_000_000, 63_500_000, 26_502_000] {
            assert!(is_listed_armor(id), "{id}");
        }
    }

    #[test]
    fn fists_are_never_removed() {
        assert!(!is_removable_weapon(0));
        assert!(!is_removable_weapon(110_000));
        assert!(is_removable_weapon(1_000_000));
        assert!(is_removable_weapon(16_000_805));
    }

    #[test]
    fn weapon_ids_split_into_infusion_and_level() {
        assert_eq!(weapon_infusion(16_000_805), 8);
        assert_eq!(weapon_level(16_000_805), 5);
        assert_eq!(weapon_infusion(3_000_000), 0);
    }

    #[test]
    fn pending_restores_survive_a_restart() {
        let saved = PendingRestores {
            armor: Some(TemporaryArmor {
                until: 100,
                character: "Ash".to_string(),
                swaps: vec![ArmorSwap { slot: 12, old_index: 4, old_id: 5, owned_before: 6, piece: 7 }],
            }),
            bombardier: Some(Bombardier {
                until: 200,
                character: "Ash".to_string(),
                weapons: vec![RemovedWeapon { slot: 1, index: 2, weapon: 3_000_000 }],
                armor: vec![ArmorSwap { slot: 13, old_index: 8, old_id: 9, owned_before: 0, piece: 79_001_000 }],
                bombs_before: 8,
                quick_slot: 2,
                quick_item: 0x4000_0097,
                quick_index: 0x8B,
                selected_before: 1,
                selection_settled: true,
                weapons_changed: false,
            }),
            stats: Some(ShuffledStats {
                until: 300,
                character: "Ash".to_string(),
                original: [12, 10, 11, 15, 13, 12, 9, 9, 7],
            }),
        };

        let text = serde_json::to_string(&saved).unwrap();
        let loaded: PendingRestores = serde_json::from_str(&text).unwrap();

        assert!(!loaded.is_empty());
        let armor = loaded.armor.unwrap();
        assert_eq!((armor.until, armor.swaps[0].piece), (100, 7));
        let bombardier = loaded.bombardier.unwrap();
        assert_eq!((bombardier.weapons[0].weapon, bombardier.quick_item), (3_000_000, 0x4000_0097));
        assert_eq!(bombardier.armor[0].piece, 79_001_000);
        assert_eq!(loaded.stats.unwrap().original[3], 15);
        assert!(PendingRestores::default().is_empty());
    }

    #[test]
    fn a_bombardier_saved_before_the_armor_still_loads() {
        let text = r#"{"bombardier":{"until":1,"character":"Ash","weapons":[],"bombs_before":0,
            "bomb_cap_before":10,"quick_slot":2,"quick_item":-1,"quick_index":-1,"selected_before":0}}"#;
        let loaded: PendingRestores = serde_json::from_str(text).unwrap();
        assert!(loaded.bombardier.unwrap().armor.is_empty());
    }

    #[test]
    fn stats_are_only_restored_if_the_total_is_unchanged() {
        let original = [12, 10, 11, 15, 13, 12, 9, 9, 7];
        assert!(can_restore_stats([7, 10, 11, 15, 13, 12, 9, 9, 12], original));
        assert!(!can_restore_stats([8, 10, 11, 15, 13, 12, 9, 9, 12], original));
    }

    #[test]
    fn bombs_go_into_a_free_slot_and_never_over_estus() {
        const ESTUS: i32 = 0x4000_00BF;
        const ASHEN: i32 = 0x4000_00C9;
        const OTHER: i32 = 0x4000_0097;
        const BOMBS: i32 = 0x4000_0129;
        const EMPTY: i32 = -1;
        assert!(is_estus(ESTUS) && is_estus(ASHEN) && !is_estus(OTHER) && !is_estus(EMPTY));
        assert_eq!(pick_bomb_slot(&[ESTUS, OTHER, EMPTY, EMPTY]), Some(2));
        assert_eq!(pick_bomb_slot(&[ESTUS, ASHEN, OTHER]), Some(2));
        assert_eq!(pick_bomb_slot(&[ESTUS, EMPTY, BOMBS]), Some(2));
        assert_eq!(pick_bomb_slot(&[ESTUS, ASHEN]), None);
    }

    #[test]
    fn bombs_the_player_already_owned_are_kept() {
        assert_eq!(bombs_to_remove(100, 0), 100);
        assert_eq!(bombs_to_remove(100, 8), 92);
        assert_eq!(bombs_to_remove(3, 8), 0);
    }

    #[test]
    fn shuffling_keeps_every_stat_value() {
        let stats = [12, 10, 11, 15, 13, 12, 9, 9, 7];
        let mut rolls = [3usize, 1, 0, 5, 2, 4, 1, 0].into_iter().cycle();
        let mut shuffled = shuffled_stats(stats, |length| rolls.next().unwrap() % length);
        assert_ne!(shuffled, stats);
        shuffled.sort_unstable();
        let mut sorted = stats;
        sorted.sort_unstable();
        assert_eq!(shuffled, sorted);
    }

    #[test]
    fn levels_follow_the_option_and_the_weapon_maximum() {
        assert_eq!(pick_level(WeaponTrait::Base, 10, 7, 3), 0);
        assert_eq!(pick_level(WeaponTrait::SameAsRightHand, 10, 7, 3), 7);
        assert_eq!(pick_level(WeaponTrait::SameAsRightHand, 5, 10, 3), 5);
        for roll in 0..50 {
            assert!(pick_level(WeaponTrait::Random, 5, 0, roll) <= 5);
        }
    }

    #[test]
    fn infusions_only_use_what_the_weapon_allows() {
        let available = [0, 1, 8];
        assert_eq!(pick_infusion(WeaponTrait::Base, &available, 8, 1), 0);
        assert_eq!(pick_infusion(WeaponTrait::SameAsRightHand, &available, 8, 1), 8);
        assert_eq!(pick_infusion(WeaponTrait::SameAsRightHand, &available, 5, 1), 0);
        assert_eq!(pick_infusion(WeaponTrait::Random, &[0], 8, 7), 0);
        assert!(available.contains(&pick_infusion(WeaponTrait::Random, &available, 0, 7)));
    }

    #[test]
    fn two_handing_counts_strength_at_one_and_a_half() {
        assert!(usable_two_handed([20, 10, 10, 10], [30, 10, 10, 10]));
        assert!(!usable_two_handed([19, 10, 10, 10], [30, 10, 10, 10]));
        assert!(!usable_two_handed([40, 9, 10, 10], [10, 10, 10, 10]));
    }

    #[test]
    fn only_base_weapons_are_picked() {
        for id in [1_000_000, 3_000_000, 11_040_000, 16_000_000] {
            assert!(is_base_weapon_id(id), "{id}");
        }
        // Fists, an upgrade, an infusion, a shield and an arrow.
        for id in [110_000, 3_000_001, 16_000_800, 21_000_000, 50_000_000] {
            assert!(!is_base_weapon_id(id), "{id}");
        }
    }

    #[test]
    fn only_idle_and_locomotion_count_as_movement() {
        for anim in [-1, 0, 20010, 20110, 20112, 20210, 22100, 27115] {
            assert!(is_movement_animation(anim), "{anim}");
        }
        for anim in [17002, 18002, 23030000, 23030320, 150032100] {
            assert!(!is_movement_animation(anim), "{anim}");
        }
    }

    #[test]
    fn speed_changes_add_together_and_stay_in_range() {
        assert_eq!(combined_speed([].into_iter()), 1.0);
        assert_eq!(combined_speed([-30, 30].into_iter()), 1.0);
        assert_eq!(combined_speed([50].into_iter()), 1.5);
        assert_eq!(combined_speed([-90, -90].into_iter()), 0.1);
        assert_eq!(combined_speed([300, 300].into_iter()), 4.0);
    }

    #[test]
    fn soul_delta_clamps_at_zero() {
        assert_eq!(apply_soul_delta(40, -100), -40);
        assert_eq!(40i32.saturating_add(apply_soul_delta(40, -100)), 0);
        assert_eq!(apply_soul_delta(500, -100), -100);
        assert_eq!(apply_soul_delta(0, -1), 0);
        assert_eq!(next_level_cost(1).round(), 673.0);
        assert_eq!(next_level_cost(11).round(), 847.0);
        assert_eq!(next_level_cost(12).round(), 1039.0);
        assert_eq!(soul_delta_for_level(-100, 10), -100);
        assert_eq!(soul_delta_for_level(-100, 1), -81);
        assert_eq!(soul_delta_for_level(-100, 50), -1822);
        assert_eq!(soul_delta_for_level(-100, 100), -7429);
        assert_eq!(soul_delta_for_level(100, 0), 81);
        assert_eq!(soul_delta_for_level(-999_999_999, 802), i32::MIN);
    }

    #[test]
    fn one_press_and_release_is_a_pulse() {
        let mut track = super::Track::default();
        for value in [0u8, 0, 1, 1, 0, 0] {
            track.observe(value);
        }
        assert_eq!(track.changes, 2);
        assert_eq!(super::change_kind(track.changes, track.first, track.current), Some("pulse"));
    }

    #[test]
    fn a_held_change_is_an_edge() {
        assert_eq!(super::change_kind(1, 0, 1), Some("edge"));
        assert_eq!(super::change_kind(0, 0, 0), None);
        assert_eq!(super::change_kind(9, 0, 0), None);
    }

    #[test]
    fn quick_slot_and_empty_item_rules() {
        assert_eq!(classify_slot(0).unwrap(), 0);
        assert_eq!(classify_slot(9).unwrap(), 9);
        assert!(classify_slot(10).is_err());
        assert!(classify_slot(-1).is_err());
        assert!(is_empty_item(-1));
        assert!(is_empty_item(0));
        assert!(!is_empty_item(0x4000_00F4));
    }

    #[test]
    fn health_percentage_clamps_and_never_rounds_alive_to_zero() {
        assert_eq!(health_at_percent(1000, 50), 500);
        assert_eq!(health_at_percent(101, 50), 50);
        assert_eq!(health_at_percent(1, 1), 1);
        assert_eq!(health_at_percent(1000, 200), 1000);
        assert_eq!(health_at_percent(1000, 0), 0);
    }
}
