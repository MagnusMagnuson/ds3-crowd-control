use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use darksouls3::app_menu::NewMenuSystem;
use darksouls3::param::{
    EQUIP_PARAM_PROTECTOR_ST, EQUIP_PARAM_WEAPON_ST, REINFORCE_PARAM_WEAPON_ST,
};
use darksouls3::sprj::{
    CSRegulationManager, GameDataMan, ItemCategory, ItemId, MenuMan, PlayerGameData, PlayerIns,
    WorldChrMan,
};
use pelite::pe64::Pe;
use serde_json::{json, Value};
use fromsoftware_shared::{FromStatic, Program};

use crate::pattern::{self, find_unique};

pub const HELD_SOULS_OFFSET: usize = 0x74;
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
struct Located {
    game_man_slot: Result<usize, String>,
    add_soul: Result<usize, String>,
    set_text_effect: Result<usize, String>,
    fade: Result<FadeSystem, String>,
    save_request: Result<usize, String>,
    equip_item: Result<usize, String>,
    find_item: Result<usize, String>,
    remove_item: Result<usize, String>,
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
        crate::log::log("resolving game patterns off the game thread");
        let located = Located {
            game_man_slot: locate_game_man_slot(),
            add_soul: find_unique(ADD_SOUL_PATTERN),
            set_text_effect: find_unique(SET_TEXT_EFFECT_PATTERN),
            fade: locate_fade_system(),
            save_request: find_unique(SAVE_REQUEST_PATTERN),
            equip_item: find_unique(EQUIP_ITEM_PATTERN),
            find_item: find_unique(FIND_ITEM_PATTERN),
            remove_item: find_unique(REMOVE_ITEM_PATTERN),
        };
        crate::log::log(format!(
            "patterns resolved: session={} souls={} text_effect={} fade={} save={} equip={}",
            located.game_man_slot.is_ok(),
            located.add_soul.is_ok(),
            located.set_text_effect.is_ok(),
            located.fade.is_ok(),
            located.save_request.is_ok(),
            located.equip_item.is_ok() && located.find_item.is_ok() && located.remove_item.is_ok(),
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

pub fn apply_soul_delta(current: i32, delta: i32) -> i32 {
    let target = (current as i64 + delta as i64).clamp(0, i32::MAX as i64);
    (target - current as i64) as i32
}

pub fn adjust_souls(delta: i32) -> Result<String, String> {
    let pgd = player_game_data()?;
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
/// Carry on anyway if the dead bit never shows up. The latest seen was frame 358.
const STAGED_DEAD_BIT_TIMEOUT_FRAMES: u32 = 480;
const STAGED_BLACK_HOLD_FRAMES: u32 = 30;
const STAGED_TRACE_FRAMES: u32 = 60;
/// Fade plate that holds the death grey. The game starts it when it sees the dead
/// bit and only clears it on the respawn path, so it stays until something resets it.
const GREY_FADE_PLATE: u32 = 6;
/// Fade plate the respawn path uses for its fade to black.
const BLACK_FADE_PLATE: u32 = 2;
/// The game's own length for both directions of the fade to black.
const BLACK_FADE_SECONDS: f32 = 0.5;
const FADE_CLEAR: [f32; 4] = [0.0; 4];
const FADE_BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
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
    elapsed: u32,
    remaining: u32,
    restoring: bool,
    restore_failures: u32,
    staged: StagedDeath,
}

/// Progress of the fake death that shows YOU DIED before standing up.
#[derive(Default)]
struct StagedDeath {
    banner_queued: bool,
    black_on: bool,
    dead_at: Option<Instant>,
    dead_bit_cleared: u32,
}

static FAKE_DEATH: Mutex<Option<FakeDeath>> = Mutex::new(None);

pub fn start_fake_death() -> Result<String, String> {
    let saved_hp = begin_fake_death()?;
    crate::log::log(format!("staged fake death started from HP {saved_hp}"));
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
        elapsed: 0,
        remaining: 0,
        restoring: false,
        restore_failures: 0,
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
    crate::log::log(format!("show you died: {message}"));
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
            fake.elapsed = fake.elapsed.saturating_add(1);
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
            crate::log::log(format!(
                "fake death restoring at frame {}",
                fake.elapsed
            ));
        }
        match restore_fake_death(fake) {
            Ok(()) => {
                fake.restore_failures = 0;
                fake.remaining = fake.remaining.saturating_sub(1);
                if fake.remaining <= FAKE_DEATH_HOLD_FRAMES - STAGED_BLACK_HOLD_FRAMES {
                    release_black(fake);
                }
                if fake.remaining == 0 {
                    crate::log::log(format!(
                        "fake death finished; HP restored to {}",
                        fake.saved_hp
                    ));
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
    let elapsed = fake.elapsed;
    let staged = &mut fake.staged;
    let Ok(player) = local_player() else {
        return false;
    };
    let player = unsafe { &mut *player };
    let player_addr = player as *mut PlayerIns as usize;

    player.super_chr_ins.modules.data.hp = 0;
    match clear_dead_event_bit(player_addr) {
        Ok(true) => {
            staged.dead_bit_cleared += 1;
            staged.dead_at.get_or_insert_with(Instant::now);
            if staged.dead_bit_cleared == 1 {
                crate::log::log(format!(
                    "staged fake death: the game set the dead bit at frame {elapsed}; cleared it"
                ));
            }
        }
        Ok(false) => {}
        Err(error) => crate::log::log(format!("staged fake death dead bit: {error}")),
    }
    if elapsed % STAGED_TRACE_FRAMES == 0 {
        crate::log::log(format!(
            "staged fake death frame={elapsed} hp={} anim={} deadBitClears={}",
            player.super_chr_ins.modules.data.hp,
            current_animation(player_addr).unwrap_or(-1),
            staged.dead_bit_cleared,
        ));
    }

    if staged.dead_at.is_none() && elapsed >= STAGED_DEAD_BIT_TIMEOUT_FRAMES {
        crate::log::log("staged fake death: no dead bit, continuing without it");
        staged.dead_at = Some(Instant::now());
    }
    let Some(dead_at) = staged.dead_at else {
        return false;
    };
    if !staged.banner_queued {
        staged.banner_queued = true;
        if let Err(error) = show_you_died() {
            crate::log::log(format!("staged fake death banner failed: {error}"));
        }
    }
    let since_dead = dead_at.elapsed();
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

fn current_animation(player: usize) -> Result<i32, String> {
    let action = module_pointer(player, ACTION_REQUEST_OFFSET)?;
    read_u32_checked(action.wrapping_add(CURRENT_ANIM_OFFSET)).map(|value| value as i32)
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
    crate::log::log("fake death: requested a save of the living player");
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
    crate::log::log(format!("fade plate {plate} -> {color:?} over {seconds}s"));
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
            crate::log::log("stamina lock ended");
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
            crate::log::log("speed changes ended by a load screen");
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
        if changes.is_empty() {
            crate::log::log("speed back to normal");
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
            crate::log::log("1 hit run ended");
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

const RIGHT_HAND_SLOT: u32 = 1;
const EQUIP_BUFFER_SIZE: usize = 0x100;
const EQUIP_BUFFER_INVENTORY_INDEX: usize = 0x38;
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

/// Gives one of `item` and equips it through the menu routine. Returns its inventory
/// index once the slot is confirmed to hold it.
fn give_and_equip(player: &mut PlayerIns, item: ItemId, slot: u32) -> Result<i32, String> {
    let located = cached()?;
    let equip_item = *located.equip_item.as_ref().map_err(|error| error.clone())?;
    let find_item = *located.find_item.as_ref().map_err(|error| error.clone())?;
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    game_data.give_item_directly(item, 1);

    let raw_id = item.into_inner();
    let player_addr = player as *mut PlayerIns as usize;
    let equipment = unsafe { &mut player.player_game_data.as_mut().equipment };
    let find: extern "C" fn(*mut std::ffi::c_void, *const u32) -> i32 =
        unsafe { std::mem::transmute(find_item) };
    let inventory = &mut equipment.equip_inventory_data as *mut _ as *mut std::ffi::c_void;
    let index = find(inventory, &raw_id);
    if index < 0 {
        return Err(format!("item {raw_id:#X} was not found in the inventory after giving it"));
    }

    let mut buffer = [0u8; EQUIP_BUFFER_SIZE];
    buffer[EQUIP_BUFFER_INVENTORY_INDEX..EQUIP_BUFFER_INVENTORY_INDEX + 4]
        .copy_from_slice(&index.to_le_bytes());
    let equip: extern "C" fn(u32, *const u8) = unsafe { std::mem::transmute(equip_item) };

    let flags_address = module_pointer(player_addr, EQUIP_UNLOCK_MODULE_OFFSET)?
        .wrapping_add(EQUIP_UNLOCK_FLAGS_OFFSET);
    let flags = read_u32_checked(flags_address)?;
    write_u32_checked(flags_address, flags | 1)?;
    equip(slot, buffer.as_ptr());
    write_u32_checked(flags_address, flags)?;

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
    let mut dlc_rows = 0;
    for (id, row) in regulation.get_param::<EQUIP_PARAM_PROTECTOR_ST>().iter() {
        let id = id as u32;
        if ASHES_OF_ARIANDEL_ARMOR.contains(&id) || RINGED_CITY_ARMOR.contains(&id) {
            dlc_rows += 1;
        }
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
    crate::log::log(format!(
        "armor pools {:?}, {dlc_rows} of {} listed DLC ids found",
        pools.iter().map(Vec::len).collect::<Vec<_>>(),
        ASHES_OF_ARIANDEL_ARMOR.len() + RINGED_CITY_ARMOR.len(),
    ));
    Ok(pools)
}

pub fn random_armor(options: ArmorOptions) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let pools = armor_pools(options.include_missing_dlc)?;

    let mut equipped = Vec::new();
    let mut removed = 0;
    let mut failure = None;
    for (slot, pool) in ARMOR_SLOTS.into_iter().zip(&pools) {
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
        let choices: Vec<u32> = pool.iter().copied().filter(|id| *id != old_id).collect();
        let Some(&piece) = choices.get(random_index(choices.len())) else {
            continue;
        };
        let item = ItemId::new(ItemCategory::Protector, piece)
            .map_err(|error| format!("armor {piece} has an invalid id: {error:?}"))?;

        match give_and_equip(player, item, slot) {
            Ok(_) => {
                equipped.push(piece);
                if options.remove_old
                    && is_real_armor(old_id)
                    && remove_inventory_entry(player, old_index)?
                {
                    removed += 1;
                }
            }
            Err(error) => failure = Some(error),
        }
    }
    match failure {
        Some(error) if equipped.is_empty() => Err(error),
        Some(error) => Ok(format!("equipped armor {equipped:?}, but one piece failed: {error}")),
        None if equipped.is_empty() => Err("no armor was found in the game data".to_string()),
        None => Ok(format!("equipped armor {equipped:?}, removed {removed} old pieces")),
    }
}

pub fn random_weapon(options: WeaponOptions) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
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
    crate::log::log(format!(
        "quick slot {slot} item {item_id:#X}: {sent}"
    ));
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

pub fn schemas_json() -> Value {
    json!([
        {
            "id": "adjust_souls",
            "name": "Adjust souls",
            "description": "Adds a signed amount to held souls. Negative values remove souls and the result is clamped at 0.",
            "arguments": [{ "name": "delta", "type": "int", "default": -100, "min": -999999999, "max": 999999999 }]
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
                { "name": "removeOld", "type": "bool", "label": "Remove previous armor", "hint": "Permanently deletes the armor you were wearing. Empty slots are skipped.", "default": 1, "min": 0, "max": 1 },
                { "name": "includeMissingDlc", "type": "bool", "label": "Include armor from DLC you do not have (experimental)", "hint": "Weapons from a missing DLC worked in a short test, but armor has not been tried.", "default": 0, "min": 0, "max": 1 }
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
            "id": "spawn_npc_invader",
            "name": "Spawn NPC invader",
            "description": "Reserved. A working invader spawn needs the game's character spawn routine, which is not called in this version.",
            "arguments": [{ "name": "npcParamIds", "type": "string", "defaultText": "" }]
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
        apply_soul_delta, armor_available, classify_slot, combined_speed, is_real_armor, health_at_percent, is_base_weapon_id,
        is_empty_item, is_listed_armor, is_movement_animation, is_removable_weapon, pick_infusion,
        pick_level,
        usable_two_handed, weapon_available,
        weapon_infusion, weapon_level, WeaponTrait,
    };

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
