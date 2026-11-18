use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use darksouls3::app_menu::NewMenuSystem;
use darksouls3::param::{
    CHARACTER_INIT_PARAM, EQUIP_PARAM_PROTECTOR_ST, EQUIP_PARAM_WEAPON_ST, MAGIC_PARAM_ST, NPC_PARAM_ST,
    SP_EFFECT_PARAM_ST, THROW_INFO_BANK,
    NPC_THINK_PARAM_ST, REINFORCE_PARAM_WEAPON_ST,
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
/// Free attunement slots: `(equip magic data, null)`. Reads only. It is the stat table plus
/// ring bonuses minus the slot length of every attuned spell.
const FREE_ATTUNE_SLOTS_PATTERN: &str =
    "48 89 5C 24 08 48 89 74 24 10 57 48 83 EC 20 48 8B FA 48 8B D9 E8 ?? ?? ?? ?? 48 8B 5B 10 8B F0 33 C0 48 85 DB 74 ?? 48 8B 9B 18 03 00 00";
/// The game's own factory for a character a map places: `(_, memory or 0, set entry, block
/// number, map data, enemy index)`. When the map entry has a CharaInit id above 0 it builds
/// a player-type NPC with gear, a face and the human AI. The debug creator cannot do that:
/// it only ever builds enemy objects, and an enemy object with the player body is an
/// invisible shell that just walks.
const MAP_CHR_FACTORY_PATTERN: &str =
    "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 41 56 41 57 48 83 EC 40 8B BC 24 88 00 00 00 48 8B 9C 24 80 00 00 00 49 8B E8 4C 8B F2 4C 8B F9 44 8B C7 48 8B CB BA 02 00 00 00 41 8B F1 E8";
/// Runs after the factory: `(_, chr, block number, map data, enemy index)`. It gives a
/// player-type NPC its NPC row, puts it at the entry's position and finishes it.
const MAP_CHR_PLACE_PATTERN: &str =
    "48 89 5C 24 08 48 89 6C 24 10 48 89 74 24 18 57 48 83 EC 20 41 8B E8 44 8B 44 24 50 48 8B DA BA 02 00 00 00 49 8B C9 49 8B F1 E8";
/// Inside the game's "a phantom left" handler, used only to find two addresses: the two
/// instructions after the constants load the message owner and call the routine
/// `(owner, message row, invade type, name slot, name object, 0)`. That routine picks a
/// line by row and invade type, puts the name in, and queues it for the message bar. It
/// frees the name.
const SUMMON_MESSAGE_SITE_PATTERN: &str =
    "41 B9 04 00 00 00 44 8B C6 BA 0E 01 00 00 48 8B 0D ?? ?? ?? ?? E8 ?? ?? ?? ?? 48 8B 0D";
const SUMMON_MESSAGE_OWNER_LOAD: usize = 14;
const SUMMON_MESSAGE_CALL: usize = 21;
/// Builds an empty name object for that routine: a tag, a wide string and a flag.
const NAME_OBJECT_PATTERN: &str =
    "48 89 4C 24 08 57 48 83 EC 30 48 C7 44 24 28 FE FF FF FF 48 89 5C 24 50 48 8B F9 C7 44 24 20 00 00 00 00 48 8D 05 ?? ?? ?? ?? 48 89 01 48 8D 59 08 48 89 5C 24 48 48 8B CB E8 ?? ?? ?? ?? 90 C6 43 28 01";
/// The game's "name of this character" routine: `(name object to fill, chr, decorated)`.
/// Lock-on, the health bar and messages all ask it. For an NPC it reads the name from the
/// text files through the NPC row, so a per-invader name is only possible by answering in
/// its place. It is plain code, the same on disk as in memory, not one of the encrypted
/// routines the game guards.
const CHR_NAME_PATTERN: &str =
    "48 8B C4 55 57 41 56 48 8D 68 98 48 81 EC 50 01 00 00 48 C7 44 24 48 FE FF FF FF 48 89 58 18 48 89 70 20 48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 45 40 41 0F B6 F0 48 8B DA 48 8B F9";
/// The game's routine for a real player joining as a phantom. Three calls inside it are
/// used on spawned NPCs: set the phantom type and refresh what follows from it (a fallback
/// when the NPC routine below is not found), get the session's list of phantoms, and add
/// a character to that list, which puts it in the list on the left of the screen.
const SUMMON_NPC_PATTERN: &str =
    "40 53 55 56 57 41 56 48 83 EC 70 48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 44 24 60 41 8B 30 48 8B E9 48 8B 0D ?? ?? ?? ?? 45 0F B6 F1 49 8B F8 48 8B DA";
const SUMMON_NPC_SET_TYPE_CALL: usize = 0x77;
const SUMMON_NPC_SESSION_CALL: usize = 0x9E;
const SUMMON_NPC_JOIN_CALL: usize = 0xEC;
/// The game's routine that turns an NPC built like a player into a phantom. Sign summons
/// and scripted invasions both go through it: `(_, chr handle, phantom type, invade type)`
/// gives back the character. Besides the type and the team it marks the NPC's player data
/// as a phantom's, which nothing else does.
const PHANTOM_NPC_PATTERN: &str =
    "44 89 44 24 18 89 54 24 10 56 48 83 EC 40 48 8B 0D ?? ?? ?? ?? 41 0F B6 F1 48 85 C9 75 2A";
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
    free_attune_slots: Result<usize, String>,
    map_chr_factory: Result<usize, String>,
    map_chr_place: Result<usize, String>,
    summon_message: Result<SummonMessage, String>,
    name_object: Result<usize, String>,
    summon_npc: Result<SummonNpc, String>,
    phantom_npc: Result<usize, String>,
}

#[derive(Clone, Copy)]
struct SummonNpc {
    /// `(chr, phantom type)`
    set_type: usize,
    /// `() -> session`
    session: usize,
    /// `(session, chr handle, phantom type, stage, -1, -1, 0)`
    join: usize,
}

#[derive(Clone, Copy)]
struct SummonMessage {
    owner_slot: usize,
    show: usize,
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
            free_attune_slots: find_unique(FREE_ATTUNE_SLOTS_PATTERN),
            map_chr_factory: find_unique(MAP_CHR_FACTORY_PATTERN),
            map_chr_place: find_unique(MAP_CHR_PLACE_PATTERN),
            summon_message: locate_summon_message(),
            name_object: find_unique(NAME_OBJECT_PATTERN),
            summon_npc: locate_summon_npc(),
            phantom_npc: find_unique(PHANTOM_NPC_PATTERN),
        };
        crate::log::log(format!(
            "patterns resolved: session={} souls={} text_effect={} fade={} save={} equip={} spawn={} attune={} invader={} invasion_message={} summon={} phantom={}",
            located.game_man_slot.is_ok(),
            located.add_soul.is_ok(),
            located.set_text_effect.is_ok(),
            located.fade.is_ok(),
            located.save_request.is_ok(),
            located.equip_item.is_ok() && located.find_item.is_ok() && located.remove_item.is_ok(),
            located.create_npc.is_ok(),
            located.free_attune_slots.is_ok(),
            located.map_chr_factory.is_ok() && located.map_chr_place.is_ok(),
            located.summon_message.is_ok() && located.name_object.is_ok(),
            located.summon_npc.is_ok(),
            located.phantom_npc.is_ok(),
        ));
        located
    });
    install_chr_name_hook();
    // Searching the game's code takes 25-50 ms each time. Done here, at attach, the first
    // ally or invader does not freeze the game for that long.
    install_locally_simulated_hook();
    ai_target_list_slot();
    let _ = npc_name_routine();
    let _ = phantom_recount();
    let _ = force_play_animation_routine();
    let _ = add_sp_effect_routine();
    let _ = erase_sp_effect_routine();
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

fn locate_summon_npc() -> Result<SummonNpc, String> {
    let site = find_unique(SUMMON_NPC_PATTERN)?;
    let call = |offset: usize| -> Result<usize, String> {
        let mut opcode = [0u8; 1];
        if copy_readable(site + offset, &mut opcode) != 1 || opcode[0] != 0xE8 {
            return Err(format!("the summon routine has no call at {offset:#x}"));
        }
        pattern::rip_target(site + offset, 1, 5)
    };
    Ok(SummonNpc {
        set_type: call(SUMMON_NPC_SET_TYPE_CALL)?,
        session: call(SUMMON_NPC_SESSION_CALL)?,
        join: call(SUMMON_NPC_JOIN_CALL)?,
    })
}

fn locate_summon_message() -> Result<SummonMessage, String> {
    let site = find_unique(SUMMON_MESSAGE_SITE_PATTERN)?;
    Ok(SummonMessage {
        owner_slot: pattern::rip_target(site + SUMMON_MESSAGE_OWNER_LOAD, 3, 7)?,
        show: pattern::rip_target(site + SUMMON_MESSAGE_CALL, 1, 5)?,
    })
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

/// `NtWriteVirtualMemory(process, address, buffer, size, written) -> status`.
type NtWriteVirtualMemory = unsafe extern "system" fn(isize, *mut std::ffi::c_void, *const std::ffi::c_void, usize, *mut usize) -> i32;

/// Windows' own write routine, which fails on memory that is not writable (read-only,
/// missing) instead of crashing. Unlike WriteProcessMemory it never makes read-only memory
/// writable, and unlike asking about the region first it does not walk every page of it.
fn nt_write_virtual_memory() -> Option<NtWriteVirtualMemory> {
    static FUNCTION: OnceLock<usize> = OnceLock::new();
    let address = *FUNCTION.get_or_init(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        GetModuleHandleW(windows::core::w!("ntdll.dll"))
            .ok()
            .and_then(|module| GetProcAddress(module, windows::core::s!("NtWriteVirtualMemory")))
            .map(|function| function as usize)
            .unwrap_or(0)
    });
    (address != 0).then(|| unsafe { std::mem::transmute::<usize, NtWriteVirtualMemory>(address) })
}

fn write_bytes_checked(addr: usize, bytes: &[u8]) -> Result<(), String> {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if addr < 0x10_000 {
        return Err(format!("address {addr:#X} is not writable"));
    }
    if bytes.is_empty() {
        return Ok(());
    }
    if let Some(write) = nt_write_virtual_memory() {
        /// The handle Windows uses for "this process".
        const THIS_PROCESS: isize = -1;
        let mut written = 0usize;
        let status = unsafe { write(THIS_PROCESS, addr as *mut _, bytes.as_ptr().cast(), bytes.len(), &mut written) };
        return if status == 0 && written == bytes.len() {
            Ok(())
        } else {
            Err(format!("address {addr:#X} is not writable"))
        };
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
/// Character byte the hit test rejects on. Bit 7 is the Invuln flag.
const INVULN_FLAG_OFFSET: usize = 0x1A09;
const INVULN_BIT: u8 = 0x80;
/// First module in the container. Its state qword is what the hit test reads.
const ACTION_MODULE_OFFSET: usize = 0;
const ACTION_HIT_STATE_OFFSET: usize = 0x58;
/// Bits 0 and 1 of that byte also make the hit test reject the player.
const ACTION_NO_HIT_BITS: u8 = 0b0000_0011;

static FREEZE_UNTIL: Mutex<Option<Instant>> = Mutex::new(None);

pub fn without_bits(value: u8, mask: u8) -> u8 {
    value & !mask
}

fn clear_bits(address: usize, mask: u8) {
    if address < 0x10_000 {
        return;
    }
    unsafe {
        let flag = address as *mut u8;
        let current = flag.read();
        let next = without_bits(current, mask);
        if next != current {
            flag.write(next);
        }
    }
}

/// A hit can raise the short "don't hit me again" flags. Their off switch waits on
/// the animation clock, which Freeze has stopped, so turn them off directly.
fn clear_stuck_hit_blocks(player: usize) {
    clear_bits(player + INVULN_FLAG_OFFSET, INVULN_BIT);
    let modules = unsafe { ((player + MODULES_OFFSET) as *const usize).read() };
    if modules < 0x10_000 {
        return;
    }
    let action = unsafe { (modules as *const usize).add(ACTION_MODULE_OFFSET).read() };
    if action < 0x10_000 {
        return;
    }
    clear_bits(action + ACTION_HIT_STATE_OFFSET, ACTION_NO_HIT_BITS);
}

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
                let player = player as usize;
                apply_speed_for_animation(player, FROZEN_SPEED, FROZEN_SPEED, now);
                clear_stuck_hit_blocks(player);
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

/// A testing switch: no hit damage until it is switched off again. Unlike the timed
/// effect it survives deaths, rests and warps.
static ALWAYS_INVINCIBLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn toggle_invincible() -> Result<String, String> {
    use std::sync::atomic::Ordering;
    let on = !ALWAYS_INVINCIBLE.fetch_xor(true, Ordering::AcqRel);
    if !on {
        // A timed Invincible that is still running keeps the protection for its own time.
        let timed = INVINCIBILITY.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
        if !timed {
            if let Ok(player) = local_player() {
                let _ = set_no_damage(unsafe { &mut *player }, false);
            }
        }
    }
    Ok(if on { "invincibility is on until you switch it off" } else { "invincibility is off" }.to_string())
}

/// Where enemy AI reads the game's own "hide the player" debug switch. The byte after it
/// is the matching "player makes no sound" switch.
const PLAYER_HIDE_PATTERN: &str = "80 3D ?? ?? ?? ?? 00 0F 94 C0 84 C0 74 0E 44 8B C3 48 8B D6 48 8B CF";

/// A testing switch: enemies neither see nor hear the player until it is switched off.
pub fn toggle_invisible() -> Result<String, String> {
    static HIDE: OnceLock<Result<usize, String>> = OnceLock::new();
    let hide = HIDE
        .get_or_init(|| find_unique(PLAYER_HIDE_PATTERN).and_then(|site| pattern::rip_target(site, 2, 7)))
        .clone()?;
    let mut now = [0u8; 1];
    if copy_readable(hide, &mut now) != 1 {
        return Err("the hide switch cannot be read".to_string());
    }
    let on = now[0] == 0;
    write_bytes_checked(hide, &[on as u8, on as u8])?;
    Ok(if on { "enemies ignore you until you switch it off" } else { "enemies notice you again" }.to_string())
}

/// Ember, the item that restores the Lord of Cinder form and with it summoning.
const EMBER: u32 = 0x1F4;
/// The game holds at most this many of one item.
const MAX_ITEM_GIFT: u32 = 99;

pub fn give_ember(amount: u32) -> Result<String, String> {
    local_player()?;
    let item = ItemId::new(ItemCategory::Goods, EMBER).map_err(|error| format!("the Ember id is invalid: {error:?}"))?;
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    let amount = amount.clamp(1, MAX_ITEM_GIFT);
    game_data.give_item_directly(item, amount);
    Ok(if amount == 1 { "gave 1 Ember".to_string() } else { format!("gave {amount} Embers") })
}

pub fn poll_invincibility() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if ALWAYS_INVINCIBLE.load(std::sync::atomic::Ordering::Acquire) {
            if let Ok(player) = local_player() {
                let _ = set_no_damage(unsafe { &mut *player }, true);
            }
            return;
        }
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
    let level = pick_level(
        options.level,
        max_weapon_level(regulation, row),
        weapon_level(current),
        random_index(MAX_WEAPON_LEVEL as usize + 1),
    );
    Ok((variant + level, candidates.len()))
}

fn max_weapon_level(regulation: &CSRegulationManager, row: &EQUIP_PARAM_WEAPON_ST) -> u32 {
    let reinforce = regulation.get_param::<REINFORCE_PARAM_WEAPON_ST>();
    let reinforce_type = u64::from(row.reinforce_type_id().max(0) as u16);
    (1..=MAX_WEAPON_LEVEL)
        .take_while(|level| reinforce.get(reinforce_type + u64::from(*level)).is_some())
        .count() as u32
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
    if let Some(active) = pending().gear_event() {
        return Err(active.to_string());
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
    firestarter: Option<Firestarter>,
    #[serde(default)]
    ooga_booga: Option<OogaBooga>,
    stats: Option<ShuffledStats>,
    #[serde(default)]
    recently_ended: Vec<EndedSwap>,
}

impl PendingRestores {
    fn is_empty(&self) -> bool {
        self.armor.is_none()
            && self.bombardier.is_none()
            && self.firestarter.is_none()
            && self.ooga_booga.is_none()
            && self.stats.is_none()
            && self.recently_ended.is_empty()
    }

    /// Bombardier and Firestarter both take the weapons and armor slots.
    fn gear_event(&self) -> Option<&'static str> {
        if self.bombardier.is_some() {
            Some(BOMBARDIER_ACTIVE)
        } else if self.firestarter.is_some() {
            Some(FIRESTARTER_ACTIVE)
        } else if self.ooga_booga.is_some() {
            Some(OOGA_BOOGA_ACTIVE)
        } else {
            None
        }
    }
}

static PENDING: Mutex<PendingRestores> =
    Mutex::new(PendingRestores {
    armor: None,
    bombardier: None,
    firestarter: None,
    ooga_booga: None,
    stats: None,
    recently_ended: Vec::new(),
});

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

/// A file save the game should not wait for.
enum FileJob {
    /// Write the text beside the file first, then swap it in, so a crash halfway cannot
    /// lose the old file.
    Replace { what: &'static str, path: std::path::PathBuf, text: String },
    Remove { what: &'static str, path: std::path::PathBuf },
}

static FILE_JOBS: OnceLock<crossbeam_channel::Sender<FileJob>> = OnceLock::new();

/// Hands a save to a thread of its own, which does them one after another in the order
/// they come. Writing a file can take a few milliseconds, which the game would feel as a
/// hitch if its own thread did it.
fn save_in_background(job: FileJob) {
    let sender = FILE_JOBS.get_or_init(|| {
        let (sender, receiver) = crossbeam_channel::unbounded::<FileJob>();
        std::thread::spawn(move || {
            crate::perf::name_thread("mod: saving files");
            for job in receiver {
                if let Err((what, error)) = run_file_job(job) {
                    crate::log::log(format!("{what} could not be saved: {error}"));
                }
            }
        });
        sender
    });
    let _ = sender.send(job);
}

fn run_file_job(job: FileJob) -> Result<(), (&'static str, String)> {
    match job {
        FileJob::Replace { what, path, text } => {
            let partial = path.with_extension("tmp");
            std::fs::write(&partial, text).map_err(|error| (what, error.to_string()))?;
            std::fs::rename(&partial, &path).map_err(|error| (what, error.to_string()))
        }
        FileJob::Remove { what, path } => match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err((what, error.to_string())),
            _ => Ok(()),
        },
    }
}

fn write_pending(restores: &PendingRestores) -> Result<(), String> {
    const WHAT: &str = "the list of gear to put back";
    let path = crate::log::app_dir()?.join(PENDING_RESTORES_FILE);
    if restores.is_empty() {
        save_in_background(FileJob::Remove { what: WHAT, path });
        return Ok(());
    }
    let text = serde_json::to_string_pretty(restores).map_err(|error| error.to_string())?;
    save_in_background(FileJob::Replace { what: WHAT, path, text });
    Ok(())
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
    if let Some(active) = slot.gear_event() {
        return Err(active.to_string());
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
        slot.remember_ended(EndedKind::Armor(active));
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
fn wear_armor_set(player: &mut PlayerIns, set: [u32; 4], event: &str) -> Vec<ArmorSwap> {
    let mut swaps = Vec::new();
    for (slot, piece) in ARMOR_SLOTS.into_iter().zip(set) {
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
            Err(error) => crate::log::log(format!("{event}: armor slot {slot} failed: {error}")),
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
    let ended = end_other_gear_swaps(&mut slot, player, &character)?;

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
    let armor = wear_armor_set(player, BOMBARDIER_ARMOR, "Bombardier");
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
        "{}Bombardier for {seconds} seconds: {removed} weapons removed, quick slot {quick_slot} holds {} bombs",
        ended_prefix(&ended),
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
        slot.remember_ended(EndedKind::Bombardier(active));
        save_pending(&slot);
    }));
}

const PYROMANCY_FLAME: u32 = 13_400_000;
/// The Fire Witch Set, in [ARMOR_SLOTS] order.
const FIRESTARTER_ARMOR: [u32; 4] = [60_000_000, 60_001_000, 60_002_000, 60_003_000];
const FIRESTARTER_ACTIVE: &str = "Firestarter is using the weapon and armor slots";
/// Goods and MagicParam rows share these ids. Combustion and Dark Orb are cut content.
/// Carthus Flame Arc is left out because it only buffs a weapon, and the flame cannot take buffs.
const BASE_PYROMANCIES: [u32; 26] = [
    2_400_000, 2_402_000, 2_403_000, 2_405_000, 2_407_000, 2_411_000, 2_420_000, 2_421_000,
    2_422_000, 2_430_000, 2_431_000, 2_436_000, 2_440_000, 2_450_000, 2_451_000,
    2_452_000, 2_453_000, 2_455_000, 2_456_000, 2_470_000, 2_562_000, 2_610_000, 2_620_000,
    2_630_000, 2_640_000, 2_650_000,
];
const ASHES_OF_ARIANDEL_PYROMANCIES: [u32; 1] = [2_457_000];
const RINGED_CITY_PYROMANCIES: [u32; 2] = [2_458_000, 2_660_000];
/// PlayerGameData field that points at the attuned spells.
const EQUIP_MAGIC_OFFSET: usize = 0x470;
/// Each entry is the MagicParam id, then a cast count left over from older games.
const MAGIC_ENTRIES_OFFSET: usize = 0x18;
const MAGIC_ENTRY_COUNT: usize = 14;
const MAGIC_SELECTED_OFFSET: usize = 0x88;
/// What the game writes into a slot it empties: id -1, count 0.
const EMPTY_MAGIC_ENTRY: u64 = 0xFFFF_FFFF;

#[derive(Deserialize, Serialize)]
struct SpellRequirement {
    spell: u32,
    intelligence: u8,
    faith: u8,
}

#[derive(Deserialize, Serialize)]
struct Firestarter {
    until: u64,
    character: String,
    weapons: Vec<RemovedWeapon>,
    armor: Vec<ArmorSwap>,
    flame: u32,
    flame_index: i32,
    /// Copies of `flame` before it was given, because only the extra copy goes at the end.
    flames_before: u32,
    spells: Vec<u32>,
    spells_given: Vec<u32>,
    /// None when the attuned spells could not be changed, so nothing is written back.
    attuned_before: Option<Vec<u64>>,
    selected_before: i32,
    requirements: Vec<SpellRequirement>,
    #[serde(skip)]
    weapons_changed: bool,
}

pub fn pyromancies_available(ashes_installed: bool, ringed_city_installed: bool) -> Vec<u32> {
    let mut spells = BASE_PYROMANCIES.to_vec();
    if ashes_installed {
        spells.extend(ASHES_OF_ARIANDEL_PYROMANCIES);
    }
    if ringed_city_installed {
        spells.extend(RINGED_CITY_PYROMANCIES);
    }
    spells
}

/// The entry the game's own attune routine writes: a count of three per use in the
/// param, or -1 when the param has none.
pub fn magic_entry(spell: u32, max_quantity: i16) -> u64 {
    let count = if max_quantity == -1 { -1 } else { i32::from(max_quantity) * 3 };
    u64::from(spell) | (u64::from(count as u32) << 32)
}

/// Takes spells in order while their slot lengths fit, at most one per entry.
pub fn pick_attunement(spells: &[(u32, u8)], free_slots: u32) -> Vec<u32> {
    let mut left = free_slots;
    let mut chosen = Vec::new();
    for (spell, length) in spells {
        if chosen.len() == MAGIC_ENTRY_COUNT {
            break;
        }
        let length = u32::from(*length);
        if length <= left {
            left -= length;
            chosen.push(*spell);
        }
    }
    chosen
}

/// The game's attunement table, used if its own routine cannot be found. Ring bonuses
/// are not counted here.
pub fn attunement_slots(attunement: u32) -> u32 {
    match attunement {
        0..=9 => 0,
        10..=13 => 1,
        14..=17 => 2,
        18..=23 => 3,
        24..=29 => 4,
        30..=39 => 5,
        40..=49 => 6,
        50..=59 => 7,
        60..=79 => 8,
        80..=98 => 9,
        _ => 10,
    }
}

fn count_owned(player: &PlayerIns, category: ItemCategory, param: u32) -> u32 {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    equipment
        .equip_inventory_data
        .items_data
        .items()
        .filter(|entry| entry.item_id.category() as u32 == category as u32 && entry.item_id.param_id() == param)
        .count() as u32
}

fn equipped_weapon(player: &PlayerIns, slot: u32) -> Option<(i32, u32)> {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    let index = equipment.equipment_indexes[slot as usize];
    u32::try_from(index)
        .ok()
        .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
        .map(|entry| (index, entry.item_id.param_id()))
}

fn random_flame() -> Result<u32, String> {
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the weapon list is unavailable: {error:?}"))?;
    let row = regulation
        .get_param::<EQUIP_PARAM_WEAPON_ST>()
        .get(u64::from(PYROMANCY_FLAME))
        .ok_or("the Pyromancy Flame is missing from the game data")?;
    let max_level = max_weapon_level(regulation, row);
    Ok(PYROMANCY_FLAME + random_index(max_level as usize + 1) as u32)
}

fn equip_magic(pgd: *mut PlayerGameData) -> Result<usize, String> {
    if pgd.is_null() {
        return Err("player data is missing".to_string());
    }
    let magic = read_usize_checked(pgd as usize + EQUIP_MAGIC_OFFSET)?;
    if magic < 0x10_000 {
        return Err("the attuned spells are missing".to_string());
    }
    Ok(magic)
}

fn read_attuned(magic: usize) -> Result<Vec<u64>, String> {
    (0..MAGIC_ENTRY_COUNT)
        .map(|entry| {
            let mut bytes = [0u8; 8];
            let address = magic + MAGIC_ENTRIES_OFFSET + entry * 8;
            if copy_readable(address, &mut bytes) != 8 {
                return Err(format!("attuned spell {entry} is not readable"));
            }
            Ok(u64::from_ne_bytes(bytes))
        })
        .collect()
}

fn write_attuned(magic: usize, entries: &[u64], selected: i32) -> Result<(), String> {
    for (entry, value) in entries.iter().enumerate().take(MAGIC_ENTRY_COUNT) {
        write_bytes_checked(magic + MAGIC_ENTRIES_OFFSET + entry * 8, &value.to_ne_bytes())?;
    }
    write_i32_checked(magic + MAGIC_SELECTED_OFFSET, selected)
}

fn free_attune_slots(player: &PlayerIns, magic: usize) -> u32 {
    let routine = cached().ok().and_then(|located| located.free_attune_slots.as_ref().ok().copied());
    match routine {
        Some(address) => {
            let free: extern "C" fn(usize, usize) -> i32 = unsafe { std::mem::transmute(address) };
            free(magic, 0).max(0) as u32
        }
        None => attunement_slots(unsafe { player.player_game_data.as_ref() }.player_info.attunement),
    }
}

/// Swaps the attuned spells for as many of `spells` as fit, in random order.
fn attune_pyromancies(player: &PlayerIns, magic: usize, spells: &[u32]) -> Result<usize, String> {
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the spell list is unavailable: {error:?}"))?;
    let params = regulation.get_param::<MAGIC_PARAM_ST>();
    let mut candidates: Vec<(u32, u8, i16)> = spells
        .iter()
        .filter_map(|spell| params.get(u64::from(*spell)).map(|row| (*spell, row.slot_length(), row.max_quantity())))
        .collect();
    for last in (1..candidates.len()).rev() {
        candidates.swap(last, random_index(last + 1));
    }

    write_attuned(magic, &[EMPTY_MAGIC_ENTRY; MAGIC_ENTRY_COUNT], 0)?;
    let free = free_attune_slots(player, magic);
    let lengths: Vec<(u32, u8)> = candidates.iter().map(|(spell, length, _)| (*spell, *length)).collect();
    let chosen = pick_attunement(&lengths, free);
    let mut entries = [EMPTY_MAGIC_ENTRY; MAGIC_ENTRY_COUNT];
    for (entry, spell) in entries.iter_mut().zip(&chosen) {
        let max_quantity = candidates.iter().find(|(id, _, _)| id == spell).map_or(-1, |(_, _, quantity)| *quantity);
        *entry = magic_entry(*spell, max_quantity);
    }
    write_attuned(magic, &entries, 0)?;
    Ok(chosen.len())
}

/// Zeroes the intelligence and faith requirements. Returns the values it replaced, or
/// nothing when `remember` is off, which is how they are kept at zero after a restart.
fn lift_requirements(spells: &[u32], remember: bool) -> Vec<SpellRequirement> {
    let Ok(regulation) = (unsafe { CSRegulationManager::instance_mut() }) else {
        return Vec::new();
    };
    let params = regulation.get_mut_param::<MAGIC_PARAM_ST>();
    let mut lifted = Vec::new();
    for spell in spells {
        let Some(row) = params.get_mut(u64::from(*spell)) else {
            continue;
        };
        if row.requirement_intellect() == 0 && row.requirement_faith() == 0 {
            continue;
        }
        if remember {
            lifted.push(SpellRequirement {
                spell: *spell,
                intelligence: row.requirement_intellect(),
                faith: row.requirement_faith(),
            });
        }
        row.set_requirement_intellect(0);
        row.set_requirement_faith(0);
    }
    lifted
}

fn restore_requirements(requirements: &[SpellRequirement]) {
    let Ok(regulation) = (unsafe { CSRegulationManager::instance_mut() }) else {
        return;
    };
    let params = regulation.get_mut_param::<MAGIC_PARAM_ST>();
    for requirement in requirements {
        if let Some(row) = params.get_mut(u64::from(requirement.spell)) {
            row.set_requirement_intellect(requirement.intelligence);
            row.set_requirement_faith(requirement.faith);
        }
    }
}

/// Takes every real weapon out of the hands. Returns them so they can go back.
fn take_weapons(player: &mut PlayerIns) -> Result<Vec<RemovedWeapon>, String> {
    let mut weapons = Vec::new();
    for weapon_slot in WEAPON_SLOTS {
        let Some((index, weapon)) = equipped_weapon(player, weapon_slot) else {
            continue;
        };
        if !is_removable_weapon(weapon) {
            continue;
        }
        call_equip(player, weapon_slot, -1)?;
        weapons.push(RemovedWeapon { slot: weapon_slot, index, weapon });
    }
    Ok(weapons)
}

fn put_weapons_back(player: &mut PlayerIns, weapons: &[RemovedWeapon], event: &str) {
    for removed in weapons {
        let result = match current_index(player, ItemCategory::Weapon, removed.weapon, removed.index) {
            Some(index) => call_equip(player, removed.slot, index),
            None => Err(format!("weapon {} is no longer owned", removed.weapon)),
        };
        if let Err(error) = result {
            crate::log::log(format!("{event}: weapon slot {} restore failed: {error}", removed.slot));
        }
    }
}

/// Ends Temporary Armor, Bombardier and Firestarter right away, so a new Bombardier or
/// Firestarter starts from the player's own gear. Returns the names of what was ended.
fn end_other_gear_swaps(
    slot: &mut PendingRestores,
    player: &mut PlayerIns,
    character: &str,
) -> Result<Vec<&'static str>, String> {
    let owners = [
        slot.armor.as_ref().map(|active| &active.character),
        slot.bombardier.as_ref().map(|active| &active.character),
        slot.firestarter.as_ref().map(|active| &active.character),
        slot.ooga_booga.as_ref().map(|active| &active.character),
    ];
    if owners.into_iter().flatten().any(|owner| owner != character) {
        return Err(OTHER_CHARACTER_WAITING.to_string());
    }
    let mut ended = Vec::new();
    if let Some(active) = slot.armor.take() {
        for swap in &active.swaps {
            if let Err(error) = restore_armor_slot(player, swap) {
                crate::log::log(format!("temporary armor slot {} restore failed: {error}", swap.slot));
            }
        }
        ended.push("Temporary Armor");
        slot.remember_ended(EndedKind::Armor(active));
    }
    if let Some(active) = slot.bombardier.take() {
        if let Err(error) = end_bombardier(player, &active) {
            crate::log::log(format!("Bombardier restore failed: {error}"));
        }
        ended.push("Bombardier");
        slot.remember_ended(EndedKind::Bombardier(active));
    }
    if let Some(active) = slot.firestarter.take() {
        if let Err(error) = end_firestarter(player, &active) {
            crate::log::log(format!("Firestarter restore failed: {error}"));
        }
        ended.push("Firestarter");
        slot.remember_ended(EndedKind::Firestarter(active));
    }
    if let Some(active) = slot.ooga_booga.take() {
        let name = ooga_name(active.big);
        if let Err(error) = end_ooga_booga(player, &active) {
            crate::log::log(format!("{name} restore failed: {error}"));
        }
        ended.push(name);
        slot.remember_ended(EndedKind::OogaBooga(active));
    }
    if !ended.is_empty() {
        save_pending(slot);
    }
    Ok(ended)
}

pub fn ended_prefix(ended: &[&str]) -> String {
    if ended.is_empty() {
        String::new()
    } else {
        format!("ended {}, then ", ended.join(" and "))
    }
}

pub fn firestarter(seconds: i32) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let until = unix_now() + seconds as u64;
    let character = read_character().1;
    let mut slot = pending();
    if let Some(active) = slot.firestarter.as_mut() {
        if active.character != character {
            return Err(OTHER_CHARACTER_WAITING.to_string());
        }
        active.until = active.until.max(until);
        save_pending(&slot);
        return Ok(format!("Firestarter extended to at least {seconds} more seconds"));
    }
    let ended = end_other_gear_swaps(&mut slot, player, &character)?;

    let flame = random_flame()?;
    let flame_item = ItemId::new(ItemCategory::Weapon, flame)
        .map_err(|error| format!("the Pyromancy Flame id is invalid: {error:?}"))?;
    let weapons = take_weapons(player)?;
    let flames_before = count_owned(player, ItemCategory::Weapon, flame);
    let flame_index = match give_and_equip(player, flame_item, RIGHT_HAND_SLOT) {
        Ok(index) => index,
        Err(error) => {
            put_weapons_back(player, &weapons, "Firestarter");
            return Err(error);
        }
    };
    let armor = wear_armor_set(player, FIRESTARTER_ARMOR, "Firestarter");

    let (ashes, ringed_city) = unsafe { darksouls3::cs::CSDlc::instance() }
        .map(|dlc| (dlc.dlc1_installed, dlc.dlc2_installed))
        .unwrap_or((false, false));
    let spells = pyromancies_available(ashes, ringed_city);
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    let mut spells_given = Vec::new();
    for spell in &spells {
        if count_owned(player, ItemCategory::Goods, *spell) > 0 {
            continue;
        }
        match ItemId::new(ItemCategory::Goods, *spell) {
            Ok(item) => {
                game_data.give_item_directly(item, 1);
                spells_given.push(*spell);
            }
            Err(error) => crate::log::log(format!("Firestarter: spell {spell} has an invalid id: {error:?}")),
        }
    }
    let requirements = lift_requirements(&spells, true);

    let pgd = player.player_game_data.as_ptr();
    let mut attuned_before = None;
    let mut selected_before = 0;
    let mut attuned = 0;
    let saved = equip_magic(pgd).and_then(|magic| {
        Ok((magic, read_attuned(magic)?, read_u32_checked(magic + MAGIC_SELECTED_OFFSET)? as i32))
    });
    match saved {
        Ok((magic, before, selected)) => match attune_pyromancies(player, magic, &spells) {
            Ok(count) => {
                attuned = count;
                attuned_before = Some(before);
                selected_before = selected;
            }
            Err(error) => {
                crate::log::log(format!("Firestarter: attuning failed: {error}"));
                let _ = write_attuned(magic, &before, selected);
            }
        },
        Err(error) => crate::log::log(format!("Firestarter: attuned spells unavailable: {error}")),
    }

    let removed = weapons.len();
    let given = spells_given.len();
    slot.firestarter = Some(Firestarter {
        until,
        character,
        weapons,
        armor,
        flame,
        flame_index,
        flames_before,
        spells,
        spells_given,
        attuned_before,
        selected_before,
        requirements,
        weapons_changed: false,
    });
    save_pending(&slot);
    Ok(format!(
        "{}Firestarter for {seconds} seconds: Pyromancy Flame +{}, {removed} weapons removed, {given} pyromancies given, {attuned} attuned",
        ended_prefix(&ended),
        weapon_level(flame),
    ))
}

/// Anything equipped from the menu comes straight off, and the flame goes back into the
/// right hand if it left. The first time, a weapon is remembered so it is not lost at the end.
fn keep_flame_only(active: &mut Firestarter) {
    let Ok(player) = local_player() else {
        return;
    };
    let player = unsafe { &mut *player };
    if keep_only_weapon(player, &mut active.weapons, active.flame, &mut active.flame_index, "Firestarter") {
        active.weapons_changed = true;
    }
}

fn end_firestarter(player: &mut PlayerIns, active: &Firestarter) -> Result<(), String> {
    // The spells go back to the player's own while the given ones are still owned.
    if let Some(before) = &active.attuned_before {
        let pgd = player.player_game_data.as_ptr();
        match equip_magic(pgd) {
            Ok(magic) => write_attuned(magic, before, active.selected_before)?,
            Err(error) => crate::log::log(format!("Firestarter: attuned spells not restored: {error}")),
        }
    }
    restore_requirements(&active.requirements);

    for weapon_slot in WEAPON_SLOTS {
        if equipped_weapon(player, weapon_slot).is_some_and(|(_, weapon)| weapon == active.flame) {
            call_equip(player, weapon_slot, -1)?;
        }
    }
    put_weapons_back(player, &active.weapons, "Firestarter");
    for swap in &active.armor {
        if let Err(error) = restore_armor_slot(player, swap) {
            crate::log::log(format!("Firestarter: armor slot {} restore failed: {error}", swap.slot));
        }
    }

    // After a crash before a save the counts are unchanged, so nothing is removed.
    if count_owned(player, ItemCategory::Weapon, active.flame) > active.flames_before {
        if let Some(index) = current_index(player, ItemCategory::Weapon, active.flame, active.flame_index) {
            remove_inventory_entry(player, index)?;
        }
    }
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    for spell in &active.spells_given {
        if count_owned(player, ItemCategory::Goods, *spell) == 0 {
            continue;
        }
        if let Ok(item) = ItemId::new(ItemCategory::Goods, *spell) {
            game_data.remove_item(item, 1);
        }
    }
    Ok(())
}

pub fn poll_firestarter() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = pending();
        let Some(active) = slot.firestarter.as_mut() else {
            return;
        };
        // A pending run from before a restart must not touch another character.
        if !load_screen_open() && unix_now() < active.until && read_character().1 == active.character {
            keep_flame_only(active);
            // The params reload with the game, so after a restart they are lifted again.
            lift_requirements(&active.spells, false);
        }
        if std::mem::take(&mut active.weapons_changed) {
            save_pending(&slot);
        }
        let Some(active) = slot.firestarter.as_ref() else {
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
        let Some(active) = slot.firestarter.take() else {
            return;
        };
        if let Err(error) = end_firestarter(unsafe { &mut *player }, &active) {
            crate::log::log(format!("Firestarter restore failed: {error}"));
        }
        slot.remember_ended(EndedKind::Firestarter(active));
        save_pending(&slot);
    }));
}

const CLUB: u32 = 8_000_000;
const GREAT_CLUB: u32 = 8_080_000;
/// Worn on the legs, so naked is the game's own naked look.
const LOINCLOTH: u32 = 76_503_000;
const LEGS_SLOT: u32 = 0xF;
const OOGA_BOOGA_ACTIVE: &str = "Ooga Booga is using the weapon and armor slots";
/// War Cry weapon arts, from the game's own names. The one with the biggest attack boost
/// that exists in the game data is used.
const WAR_CRY_EFFECTS: [i32; 7] = [1_062_301, 1_070_101, 1_070_401, 1_070_501, 1_080_001, 1_080_601, 120_000];

#[derive(Deserialize, Serialize)]
struct OogaBooga {
    until: u64,
    character: String,
    big: bool,
    weapons: Vec<RemovedWeapon>,
    armor: Vec<ArmorSwap>,
    club: u32,
    club_index: i32,
    clubs_before: u32,
    /// The club row as it was, so the lifted requirements and the buff can go back.
    row_before: Option<ClubRow>,
    #[serde(skip)]
    changed: bool,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
struct ClubRow {
    strength: u8,
    dexterity: u8,
    intelligence: u8,
    faith: u8,
    resident_effect: i32,
}

fn ooga_name(big: bool) -> &'static str {
    if big {
        "Big Ooga Booga"
    } else {
        "Ooga Booga"
    }
}

/// The War Cry effect with the biggest physical attack boost, if any is in the game data.
fn war_cry_effect() -> Option<i32> {
    let regulation = unsafe { CSRegulationManager::instance() }.ok()?;
    let effects = regulation.get_param::<SP_EFFECT_PARAM_ST>();
    WAR_CRY_EFFECTS
        .iter()
        .filter_map(|id| effects.get(*id as u64).map(|row| (*id, row.physics_attack_power_rate())))
        .filter(|(_, rate)| rate.is_finite())
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(id, _)| id)
}

/// No stat requirements, and War Cry's boost for as long as the club is held. Returns the
/// row as it was, or nothing when `remember` is off, which is how it is set again after
/// the params reload.
fn boost_club(base: u32, effect: Option<i32>, remember: bool) -> Option<ClubRow> {
    let regulation = unsafe { CSRegulationManager::instance_mut() }.ok()?;
    let row = regulation.get_mut_param::<EQUIP_PARAM_WEAPON_ST>().get_mut(u64::from(base))?;
    let before = ClubRow {
        strength: row.proper_strength(),
        dexterity: row.proper_agility(),
        intelligence: row.proper_magic(),
        faith: row.proper_faith(),
        resident_effect: row.resident_sp_effect_id0(),
    };
    row.set_proper_strength(0);
    row.set_proper_agility(0);
    row.set_proper_magic(0);
    row.set_proper_faith(0);
    if let Some(effect) = effect {
        row.set_resident_sp_effect_id0(effect);
    }
    remember.then_some(before)
}

fn restore_club(base: u32, before: &ClubRow) {
    let Ok(regulation) = (unsafe { CSRegulationManager::instance_mut() }) else {
        return;
    };
    if let Some(row) = regulation.get_mut_param::<EQUIP_PARAM_WEAPON_ST>().get_mut(u64::from(base)) {
        row.set_proper_strength(before.strength);
        row.set_proper_agility(before.dexterity);
        row.set_proper_magic(before.intelligence);
        row.set_proper_faith(before.faith);
        row.set_resident_sp_effect_id0(before.resident_effect);
    }
}

fn club_base(big: bool) -> u32 {
    if big {
        GREAT_CLUB
    } else {
        CLUB
    }
}

/// Takes off every armor piece but the legs, which get the Loincloth. Remembers in `swaps`
/// what each slot needs to get its own armor back. Returns whether `swaps` changed.
fn strip_armor(player: &mut PlayerIns, swaps: &mut Vec<ArmorSwap>, event: &str) -> bool {
    let mut changed = wear_loincloth(player, swaps, event);
    for slot in ARMOR_SLOTS.into_iter().filter(|slot| *slot != LEGS_SLOT) {
        let equipment = unsafe { &player.player_game_data.as_ref().equipment };
        let old_index = equipment.equipment_indexes[slot as usize];
        let old_id = u32::try_from(old_index)
            .ok()
            .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
            .map_or(0, |entry| entry.item_id.param_id());
        if !is_real_armor(old_id) {
            continue;
        }
        if let Err(error) = call_equip(player, slot, -1) {
            crate::log::log(format!("{event}: could not take off armor slot {slot}: {error}"));
            continue;
        }
        // Only the first piece taken from a slot is the player's own.
        if !swaps.iter().any(|swap| swap.slot == slot) {
            swaps.push(ArmorSwap { slot, old_index, old_id, owned_before: 0, piece: 0 });
            changed = true;
        }
    }
    changed
}

/// Puts the Loincloth on the legs, giving one only the first time.
fn wear_loincloth(player: &mut PlayerIns, swaps: &mut Vec<ArmorSwap>, event: &str) -> bool {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    let old_index = equipment.equipment_indexes[LEGS_SLOT as usize];
    let old_id = u32::try_from(old_index)
        .ok()
        .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
        .map_or(0, |entry| entry.item_id.param_id());
    if old_id == LOINCLOTH {
        return false;
    }
    let item = match ItemId::new(ItemCategory::Protector, LOINCLOTH) {
        Ok(item) => item,
        Err(error) => {
            crate::log::log(format!("{event}: the Loincloth id is invalid: {error:?}"));
            return false;
        }
    };
    let owned_before = count_armor(player, LOINCLOTH);
    let result = match spare_armor_index(player, LOINCLOTH) {
        Some(index) => call_equip(player, LEGS_SLOT, index),
        None => give_and_equip(player, item, LEGS_SLOT).map(|_| ()),
    };
    if let Err(error) = result {
        crate::log::log(format!("{event}: could not put on the Loincloth: {error}"));
        return false;
    }
    match swaps.iter_mut().find(|swap| swap.slot == LEGS_SLOT) {
        // Already remembered: only the leg armor put on during the event comes off.
        Some(_) => false,
        None => {
            swaps.push(ArmorSwap { slot: LEGS_SLOT, old_index, old_id, owned_before, piece: LOINCLOTH });
            true
        }
    }
}

pub fn ooga_booga(seconds: i32, big: bool) -> Result<String, String> {
    let name = ooga_name(big);
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let until = unix_now() + seconds as u64;
    let character = read_character().1;
    let mut slot = pending();
    if let Some(active) = slot.ooga_booga.as_mut() {
        if active.character != character {
            return Err(OTHER_CHARACTER_WAITING.to_string());
        }
        if active.big == big {
            active.until = active.until.max(until);
            save_pending(&slot);
            return Ok(format!("{name} extended to at least {seconds} more seconds"));
        }
    }
    let ended = end_other_gear_swaps(&mut slot, player, &character)?;

    let base = club_base(big);
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the weapon list is unavailable: {error:?}"))?;
    let row = regulation
        .get_param::<EQUIP_PARAM_WEAPON_ST>()
        .get(u64::from(base))
        .ok_or_else(|| format!("weapon {base} is missing from the game data"))?;
    let club = base + max_weapon_level(regulation, row);
    let club_item = ItemId::new(ItemCategory::Weapon, club)
        .map_err(|error| format!("the club id is invalid: {error:?}"))?;

    let effect = war_cry_effect();
    let row_before = boost_club(base, effect, true);
    let weapons = take_weapons(player)?;
    let clubs_before = count_owned(player, ItemCategory::Weapon, club);
    let club_index = match give_and_equip(player, club_item, RIGHT_HAND_SLOT) {
        Ok(index) => index,
        Err(error) => {
            put_weapons_back(player, &weapons, name);
            if let Some(before) = &row_before {
                restore_club(base, before);
            }
            return Err(error);
        }
    };
    let mut armor = Vec::new();
    strip_armor(player, &mut armor, name);

    let removed = weapons.len();
    let stripped = armor.len();
    slot.ooga_booga = Some(OogaBooga {
        until,
        character,
        big,
        weapons,
        armor,
        club,
        club_index,
        clubs_before,
        row_before,
        changed: false,
    });
    save_pending(&slot);
    let buff = match effect {
        Some(effect) => format!("War Cry boost {effect}"),
        None => "no War Cry boost was found in the game data".to_string(),
    };
    Ok(format!(
        "{}{name} for {seconds} seconds: weapon {club} (+{}), {removed} weapons and {stripped} armor pieces taken off, {buff}",
        ended_prefix(&ended),
        weapon_level(club),
    ))
}

/// The club stays in the right hand, nothing else goes in the hands, and no armor goes on.
fn keep_club_only(active: &mut OogaBooga) {
    let Ok(player) = local_player() else {
        return;
    };
    let player = unsafe { &mut *player };
    let name = ooga_name(active.big);
    if keep_only_weapon(player, &mut active.weapons, active.club, &mut active.club_index, name) {
        active.changed = true;
    }
    if strip_armor(player, &mut active.armor, name) {
        active.changed = true;
    }
}

/// Anything equipped from the menu comes straight off, and `weapon` goes back into the
/// right hand if it left. The first time, a weapon is remembered so it is not lost at the
/// end. Returns whether anything to remember changed.
fn keep_only_weapon(
    player: &mut PlayerIns,
    weapons: &mut Vec<RemovedWeapon>,
    weapon_id: u32,
    weapon_index: &mut i32,
    event: &str,
) -> bool {
    let mut changed = false;
    for weapon_slot in WEAPON_SLOTS {
        let Some((index, weapon)) = equipped_weapon(player, weapon_slot) else {
            continue;
        };
        if weapon == weapon_id || !is_removable_weapon(weapon) {
            continue;
        }
        if let Err(error) = call_equip(player, weapon_slot, -1) {
            crate::log::log(format!("{event}: could not clear slot {weapon_slot}: {error}"));
            continue;
        }
        if !weapons.iter().any(|removed| removed.slot == weapon_slot) {
            weapons.push(RemovedWeapon { slot: weapon_slot, index, weapon });
            changed = true;
        }
    }
    if equipped_weapon(player, RIGHT_HAND_SLOT).is_some_and(|(_, weapon)| weapon == weapon_id) {
        return changed;
    }
    let index = match current_index(player, ItemCategory::Weapon, weapon_id, *weapon_index) {
        Some(index) => call_equip(player, RIGHT_HAND_SLOT, index).map(|_| index),
        None => ItemId::new(ItemCategory::Weapon, weapon_id)
            .map_err(|error| format!("{error:?}"))
            .and_then(|item| give_and_equip(player, item, RIGHT_HAND_SLOT)),
    };
    match index {
        Ok(index) if index != *weapon_index => {
            *weapon_index = index;
            changed = true;
        }
        Ok(_) => {}
        Err(error) => crate::log::log(format!("{event}: could not equip the weapon again: {error}")),
    }
    changed
}

fn end_ooga_booga(player: &mut PlayerIns, active: &OogaBooga) -> Result<(), String> {
    let name = ooga_name(active.big);
    // The club comes off before its row loses the War Cry effect, so the game takes off
    // the same effect it put on.
    for weapon_slot in WEAPON_SLOTS {
        if equipped_weapon(player, weapon_slot).is_some_and(|(_, weapon)| weapon == active.club) {
            call_equip(player, weapon_slot, -1)?;
        }
    }
    if let Some(before) = &active.row_before {
        restore_club(club_base(active.big), before);
    }
    put_weapons_back(player, &active.weapons, name);
    for swap in &active.armor {
        if let Err(error) = restore_armor_slot(player, swap) {
            crate::log::log(format!("{name}: armor slot {} restore failed: {error}", swap.slot));
        }
    }
    // After a crash before a save the count is unchanged, so nothing is removed.
    if count_owned(player, ItemCategory::Weapon, active.club) > active.clubs_before {
        if let Some(index) = current_index(player, ItemCategory::Weapon, active.club, active.club_index) {
            remove_inventory_entry(player, index)?;
        }
    }
    Ok(())
}

pub fn poll_ooga_booga() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = pending();
        let Some(active) = slot.ooga_booga.as_mut() else {
            return;
        };
        // A pending run from before a restart must not touch another character.
        if !load_screen_open() && unix_now() < active.until && read_character().1 == active.character {
            keep_club_only(active);
            // The params reload with the game, so after a restart they are changed again.
            if active.row_before.is_some() {
                boost_club(club_base(active.big), war_cry_effect(), false);
            }
        }
        if std::mem::take(&mut active.changed) {
            save_pending(&slot);
        }
        let Some(active) = slot.ooga_booga.as_ref() else {
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
        let Some(active) = slot.ooga_booga.take() else {
            return;
        };
        if let Err(error) = end_ooga_booga(unsafe { &mut *player }, &active) {
            crate::log::log(format!("{} restore failed: {error}", ooga_name(active.big)));
        }
        slot.remember_ended(EndedKind::OogaBooga(active));
        save_pending(&slot);
    }));
}

/// How long a finished swap is kept in this run of the game. The game saves the gear well
/// before this, so after it closing the game can no longer bring the swap back.
const KEEP_ENDED_SWAP: Duration = Duration::from_secs(15 * 60);

/// A swap that was put back. Closing the game with its window's X skips the last save, so
/// the next start can load a save from the middle of the swap. The record is kept, and if
/// the swap is visibly back on the first load, it is put back once more.
#[derive(Deserialize, Serialize)]
enum EndedKind {
    Armor(TemporaryArmor),
    Bombardier(Bombardier),
    Firestarter(Firestarter),
    OogaBooga(OogaBooga),
    Stats(ShuffledStats),
}

#[derive(Deserialize, Serialize)]
struct EndedSwap {
    kind: EndedKind,
    /// When it ended in this run of the game. None for one from an earlier run.
    #[serde(skip)]
    here: Option<Instant>,
}

impl EndedKind {
    fn character(&self) -> &str {
        match self {
            Self::Armor(active) => &active.character,
            Self::Bombardier(active) => &active.character,
            Self::Firestarter(active) => &active.character,
            Self::OogaBooga(active) => &active.character,
            Self::Stats(active) => &active.character,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Armor(_) => "Temporary Armor",
            Self::Bombardier(_) => "Bombardier",
            Self::Firestarter(_) => "Firestarter",
            Self::OogaBooga(active) => ooga_name(active.big),
            Self::Stats(_) => "Shuffle Stats",
        }
    }

    /// Whether the loaded character still shows the swap: its weapon in hand, its armor
    /// worn or its stats shuffled.
    fn is_back(&self, player: &PlayerIns) -> bool {
        let holds = |id: u32| {
            WEAPON_SLOTS.into_iter().any(|slot| equipped_weapon(player, slot).is_some_and(|(_, weapon)| weapon == id))
        };
        let wears = |swaps: &[ArmorSwap]| {
            let equipment = unsafe { &player.player_game_data.as_ref().equipment };
            swaps.iter().filter(|swap| swap.piece != 0).any(|swap| {
                u32::try_from(equipment.equipment_indexes[swap.slot as usize])
                    .ok()
                    .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
                    .is_some_and(|entry| is_armor(entry, swap.piece))
            })
        };
        match self {
            Self::Armor(active) => wears(&active.swaps),
            Self::Bombardier(active) => wears(&active.armor),
            Self::Firestarter(active) => holds(active.flame),
            Self::OogaBooga(active) => holds(active.club),
            Self::Stats(active) => {
                let current = read_stats(unsafe { &player.player_game_data.as_ref().player_info });
                current != active.original && can_restore_stats(current, active.original)
            }
        }
    }

    fn put_back(&self, player: &mut PlayerIns) -> Result<(), String> {
        match self {
            Self::Armor(active) => {
                for swap in &active.swaps {
                    restore_armor_slot(player, swap)?;
                }
                Ok(())
            }
            Self::Bombardier(active) => end_bombardier(player, active),
            Self::Firestarter(active) => end_firestarter(player, active),
            Self::OogaBooga(active) => end_ooga_booga(player, active),
            Self::Stats(active) => apply_stats(player, active.original),
        }
    }
}

impl PendingRestores {
    fn remember_ended(&mut self, kind: EndedKind) {
        self.recently_ended.push(EndedSwap { kind, here: Some(Instant::now()) });
    }
}

/// Forgets finished swaps of this run once the game has surely saved, and checks the ones
/// from an earlier run the first time their character is in the world.
pub fn poll_recently_ended() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = pending();
        if slot.recently_ended.is_empty() {
            return;
        }
        let before = slot.recently_ended.len();
        slot.recently_ended
            .retain(|ended| ended.here.is_none_or(|at| at.elapsed() < KEEP_ENDED_SWAP));
        let mut changed = slot.recently_ended.len() != before;

        if slot.recently_ended.iter().any(|ended| ended.here.is_none()) {
            let fake_death = FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_some();
            if let Ok(player) = local_player() {
                let player = unsafe { &mut *player };
                let alive = player.super_chr_ins.modules.data.hp > 0;
                let (loaded, character) = read_character();
                if loaded && can_restore_armor(load_screen_open(), true, alive, fake_death) {
                    let (mine, rest): (Vec<EndedSwap>, Vec<EndedSwap>) = std::mem::take(&mut slot.recently_ended)
                        .into_iter()
                        .partition(|ended| ended.here.is_none() && ended.kind.character() == character);
                    slot.recently_ended = rest;
                    // Newest first, so older swaps end up with the gear they started from.
                    for ended in mine.iter().rev() {
                        if !ended.kind.is_back(player) {
                            continue;
                        }
                        let name = ended.kind.name();
                        crate::log::log(format!("{name}: the game loaded a save from the middle of it, putting things back again"));
                        if let Err(error) = ended.kind.put_back(player) {
                            crate::log::log(format!("{name} restore failed: {error}"));
                        }
                    }
                    changed |= !mine.is_empty();
                }
            }
        }
        if changed {
            save_pending(&slot);
        }
    }));
}

// ---- Mimic Chests: three chests, one holds a prize, the others are Mimics. ----

/// Weapons the winning chest can hold, from the game's own names. They are given fully
/// upgraded and without an infusion.
const PRIZE_WEAPONS: [u32; 28] = [
    6_020_000,  // Claymore
    2_060_000,  // Lothric Knight Sword
    6_070_000,  // Astora Greatsword
    6_240_000,  // Dark Sword
    5_000_000,  // Uchigatana
    6_250_000,  // Black Knight Sword
    6_200_000,  // Farron Greatsword
    6_150_000,  // Exile Greatsword
    10_050_000, // Black Knight Greataxe
    6_180_000,  // Profaned Greatsword
    16_000_000, // Sellsword Twinblades
    5_010_000,  // Washing Pole
    6_330_000,  // Drakeblood Greatsword
    6_310_000,  // Hollowslayer Greatsword
    2_140_000,  // Irithyll Straight Sword
    6_080_000,  // Murakumo
    6_170_000,  // Greatsword of Judgment
    6_270_000,  // Twin Princes' Greatsword
    9_160_000,  // Dragonslayer Swordspear
    4_070_000,  // Pontiff Knight Curved Sword
    16_090_000, // Onikiri and Ubadachi
    10_150_000, // Black Knight Glaive
    6_300_000,  // Wolf Knight's Greatsword
    7_070_000,  // Butcher Knife
    8_170_000,  // Gargoyle Flame Hammer
    6_280_000,  // Lothric's Holy Sword
    4_080_000,  // Storm Curved Sword
    6_320_000,  // Moonlight Greatsword
];
const ASHES_OF_ARIANDEL_PRIZE_WEAPONS: [u32; 1] = [10_180_000]; // Friede's Great Scythe
const RINGED_CITY_PRIZE_WEAPONS: [u32; 2] = [2_250_000, 2_260_000]; // Ringed Knight Straight Sword, Gael's Greatsword
/// Rings, in their strongest version.
const PRIZE_RINGS: [u32; 23] = [
    20_023, // Havel's Ring+3
    20_033, // Ring of Favor+3
    20_013, // Chloranthy Ring+3
    20_003, // Life Ring+3
    20_380, // Hornet Ring
    20_251, // Ring of the Sun's First Born
    20_272, // Lingering Dragoncrest Ring+2
    20_300, // Dusk Crown Ring
    20_510, // Fire Clutch Ring
    20_660, // Lloyd's Sword Ring
    20_750, // Pontiff's Right Eye
    20_480, // Knight Slayer's Ring
    20_082, // Dark Stoneplate Ring+2
    20_043, // Ring of Steel Protection+3
    20_240, // Great Swamp Ring
    20_241, // Witch's Ring
    20_282, // Sage Ring+2
    20_250, // Morne's Ring
    20_450, // Carthus Milkring
    20_370, // Hawk Ring
    20_393, // Covetous Gold Serpent Ring+3
    20_193, // Wolf Ring+3
    20_200, // Leo Ring
];

/// The Mimic's own animations, asked for through the force-play routine. A sleeping Mimic
/// shows them with 2000000 added. 3000 starts opening the lid; 4100 after it is the bite
/// from the chest (the Mimicry grab), and 3004 after it makes it stand up instead.
const MIMIC_OPEN: i32 = 3000;
const MIMIC_BITE: i32 = 4100;
const MIMIC_STAND: i32 = 3004;
/// Frames between opening the lid and the bite or standing up. Tested at 45.
const MIMIC_FOLLOW_FRAMES: u32 = 45;
/// How close and how straight ahead a chest must be for the interact button to open it.
const CHEST_REACH_METRES: f32 = 1.8;
const CHEST_FACING_COSINE: f32 = 0.6;
/// How long the chests may take to appear before the game gives up.
const CHEST_SPAWN_SECONDS: u64 = 30;
/// After the pick, the game lingers this long before it lets go of the chests.
const CHEST_AFTERMATH_SECONDS: u64 = 8;
/// Controls are locked from opening the wrong chest until the bite has caught the player.
const BITE_LOCK_SECONDS: i32 = 2;
/// The bitten Mimic wakes once the bite is over, so it fights on after.
const AFTER_BITE_FRAMES: u32 = 240;
/// Stands for "wake it up" in the list of things to do later.
const WAKE: i32 = -1;
/// Stands for "log its effects" in the list of things to do later.
const LOG_EFFECTS: i32 = -2;
/// A on an Xbox controller, interact on the game's default controls.
const XINPUT_GAMEPAD_A: u16 = 0x1000;
/// E, interact on the game's default keyboard controls.
const KEY_E: i32 = 0x45;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrizeKind {
    Either,
    Weapon,
    Ring,
}

impl PrizeKind {
    pub fn from_index(index: i64) -> Self {
        match index {
            1 => Self::Weapon,
            2 => Self::Ring,
            _ => Self::Either,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChestPhase {
    /// Waiting for the three Mimics to appear.
    Spawning,
    /// Three chests stand there; one press of interact in front of one opens it.
    Waiting,
    /// A chest was picked or the time ran out; the animations finish, then it is over.
    Done,
}

struct ChestGame {
    phase: ChestPhase,
    started: Instant,
    seconds: u64,
    prize: PrizeKind,
    spots: Vec<([f32; 3], f32)>,
    chests: Vec<usize>,
    place_frames: u32,
    winner: usize,
    full_health: Vec<i32>,
    interact_was_down: bool,
    /// Animations to ask for later: character, animation, frames left.
    later: Vec<(usize, i32, u32)>,
    done_at: Option<Instant>,
}

/// The routine behind the game's SetEventSpecialEffect command: (character, effect id).
/// The event command reaches it through an object only map-placed characters have, so the
/// mod calls it directly. A near copy of it exists, so the pattern is long.
const ADD_SP_EFFECT_PATTERN: &str = "48 8B C4 48 89 58 08 48 89 68 10 48 89 70 18 48 89 78 20 41 56 48 81 EC 90 00 00 00 C7 40 B8 00 00 80 3F C7 40 BC 00 00 80 3F C7 40 C0 00 00 80 3F 0F 29 70 E8 F3 0F 10 35 D3 F8 4E 03 C7 40 C4 00 00 80 3F 45 32 F6 F6 81 0C 1A 00 00 40 0F 28 40 B8 66 0F 7F 40 B8 8B EA 48 8B F9 C7 40 C8 00 00 80 3F C7 40 CC 00 00 80 3F C7 40 D0 00 00 80 3F C7 40 D4 00 00 80 3F C7 40 D8 00 00 80 3F C7 40 DC 00 00 80 3F C7 40 E0 00 00 80 3F 40 B6 01 74 77 E8 59 DA FF FF 84 C0 0F";
/// The routine behind EraseEventSpecialEffect: (character, effect id).
const ERASE_SP_EFFECT_PATTERN: &str = "48 8B 89 C8 11 00 00 E9 ?? ?? ?? ?? CC 48 8B 4B 48";

fn add_sp_effect_routine() -> Result<usize, String> {
    static ROUTINE: OnceLock<Result<usize, String>> = OnceLock::new();
    ROUTINE.get_or_init(|| find_unique(ADD_SP_EFFECT_PATTERN)).clone()
}

fn erase_sp_effect_routine() -> Result<usize, String> {
    static ROUTINE: OnceLock<Result<usize, String>> = OnceLock::new();
    ROUTINE.get_or_init(|| find_unique(ERASE_SP_EFFECT_PATTERN)).clone()
}

/// Puts a special effect on a character, as the game's event scripts do.
fn add_sp_effect(chr: usize, effect: i32) -> Result<(), String> {
    let add: extern "C" fn(usize, i32) -> u8 = unsafe { std::mem::transmute(add_sp_effect_routine()?) };
    add(chr, effect);
    Ok(())
}

/// Takes a special effect off a character, as the game's event scripts do.
fn erase_sp_effect(chr: usize, effect: i32) -> Result<(), String> {
    let erase: extern "C" fn(usize, i32) -> u8 = unsafe { std::mem::transmute(erase_sp_effect_routine()?) };
    erase(chr, effect);
    Ok(())
}

/// A Mimic that was hit and fights has effect 5400 where a sleeping one has 5405; the
/// rest of its effects, its "Sleep State" (11720) included, stay the same. Swapping the
/// two is what a hit does to it.
const MIMIC_DORMANT_EFFECT: i32 = 5_405;
const MIMIC_FIGHTING_EFFECT: i32 = 5_400;

/// ChrIns: the special effect holder. Its list of active effects starts at +8; each entry
/// has the effect id at +0x60 and the next entry at +0x78.
const CHR_SP_EFFECTS: usize = 0x11C8;
const SP_EFFECT_LIST: usize = 0x08;
const SP_EFFECT_ENTRY_ID: usize = 0x60;
const SP_EFFECT_ENTRY_NEXT: usize = 0x78;

/// The ids of the effects on a character right now.
fn chr_sp_effects(chr: usize) -> Vec<i32> {
    let mut ids = Vec::new();
    let Ok(holder) = read_usize_checked(chr + CHR_SP_EFFECTS) else {
        return ids;
    };
    let mut entry = read_usize_checked(holder + SP_EFFECT_LIST).unwrap_or(0);
    while entry >= 0x10_000 && ids.len() < 64 {
        ids.push(read_u32_checked(entry + SP_EFFECT_ENTRY_ID).map_or(-1, |id| id as i32));
        entry = read_usize_checked(entry + SP_EFFECT_ENTRY_NEXT).unwrap_or(0);
    }
    ids
}

fn wake_mimic(chr: usize) -> Result<(), String> {
    erase_sp_effect(chr, MIMIC_DORMANT_EFFECT)?;
    add_sp_effect(chr, MIMIC_FIGHTING_EFFECT)
}

static CHEST_GAME: Mutex<Option<ChestGame>> = Mutex::new(None);

/// Mimics side by side in a straight row in front of the player, all turned the same
/// way: back toward the player, like chests standing against a wall.
fn chest_row_spots() -> Result<Vec<([f32; 3], f32)>, String> {
    let (ahead, _) = invader_spawn_point(MIMIC_DISTANCE)?;
    let here = local_player().ok().and_then(|player| chr_position(player as usize)).unwrap_or(ahead);
    let convention = player_facing_convention()?;
    // Sideways is across the real way ahead, which is where the middle chest stands.
    let (dx, dz) = (ahead[0] - here[0], ahead[2] - here[2]);
    let length = (dx * dx + dz * dz).sqrt().max(0.01);
    let right = [dz / length, -dx / length];
    // The middle chest's way to the player; the others face the same way, not the player.
    let turn = facing_for(convention, here[0] - ahead[0], here[2] - ahead[2]);
    Ok(MIMIC_ROW
        .iter()
        .map(|side| ([ahead[0] + right[0] * side, ahead[1], ahead[2] + right[1] * side], turn))
        .collect())
}

/// Writes each character's spot and turn. The game's physics can put a character back on
/// its first frames, so this runs for a few frames in a row.
fn place_row(chrs: &[usize], spots: &[([f32; 3], f32)]) {
    for (chr, (spot, facing)) in chrs.iter().zip(spots) {
        let Ok(physics) = module_pointer(*chr, PHYSICS_MODULE_OFFSET) else {
            continue;
        };
        for (axis, value) in spot.iter().enumerate() {
            let _ = write_bytes_checked(physics + PHYSICS_POSITION + axis * 4, &value.to_ne_bytes());
        }
        let _ = write_bytes_checked(physics + PHYSICS_ROTATION + 4, &facing.to_ne_bytes());
    }
}

/// Three Mimic spawns with the Mimic's own AI.
fn mimic_picks() -> Result<Vec<EnemyChoice>, String> {
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the NPC list is unavailable: {error:?}"))?;
    if regulation.get_param::<NPC_PARAM_ST>().get(u64::from(MIMIC)).is_none() {
        return Err(format!("NPC {MIMIC} is missing from the game data"));
    }
    let think_ids: Vec<u32> = regulation
        .get_param::<NPC_THINK_PARAM_ST>()
        .iter()
        .filter(|(id, _)| *id > 0)
        .map(|(id, _)| id as u32)
        .collect();
    let think_id = think_candidates(MIMIC, &think_ids)
        .into_iter()
        .next()
        .ok_or("the Mimic has no AI row")?;
    Ok(MIMIC_ROW
        .iter()
        .map(|_| EnemyChoice { npc_id: MIMIC, think_id, invader: false, label: 0, friendly: false })
        .collect())
}

fn queue_spawns(picks: Vec<EnemyChoice>) -> Result<(), String> {
    let routine = create_npc_address()?;
    let creator = debug_creator()?;
    let mut slot = ENEMY_QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
    match slot.as_mut() {
        Some(queue) => {
            queue.remaining.extend(picks.iter().copied());
            queue.replacements = 0;
        }
        None => {
            *slot = Some(EnemyQueue {
                creator,
                routine,
                remaining: picks.clone(),
                pool: picks,
                replace_oldest: true,
                limit: 5,
                full_frames: 0,
                pause_frames: 0,
                replacements: 0,
                checks: Vec::new(),
                in_flight: None,
                started: 0,
            });
        }
    }
    Ok(())
}

pub fn mimic_chests(seconds: i32, prize: PrizeKind) -> Result<String, String> {
    let player = unsafe { &*local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    let mut slot = CHEST_GAME.lock().unwrap_or_else(|poison| poison.into_inner());
    if slot.is_some() {
        return Err("three chests are already waiting".to_string());
    }
    force_play_animation_routine()?;
    add_sp_effect_routine()?;
    erase_sp_effect_routine()?;
    let picks = mimic_picks()?;
    let spots = chest_row_spots()?;
    // Only Mimics made from here on count: the queue reports each one it makes.
    NEW_MIMICS.lock().unwrap_or_else(|poison| poison.into_inner()).clear();
    queue_spawns(picks)?;
    *slot = Some(ChestGame {
        phase: ChestPhase::Spawning,
        started: Instant::now(),
        seconds: seconds.max(1) as u64,
        prize,
        spots,
        chests: Vec::new(),
        place_frames: 0,
        winner: random_index(MIMIC_ROW.len()),
        full_health: Vec::new(),
        interact_was_down: false,
        later: Vec::new(),
        done_at: None,
    });
    Ok(format!("three chests for {seconds} seconds, one holds a prize"))
}

/// Interact on a controller (A) or the keyboard (E, only while the game has focus).
fn interact_down() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    if crate::input::controller_buttons() & XINPUT_GAMEPAD_A != 0 {
        return true;
    }
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(GetForegroundWindow(), Some(&mut owner)) };
    owner == unsafe { GetCurrentProcessId() } && unsafe { GetAsyncKeyState(KEY_E) } as u16 & 0x8000 != 0
}

fn player_forward(player: usize) -> Option<[f32; 2]> {
    let ctrl = read_usize_checked(player + CHR_CTRL_OFFSET).ok()?;
    let x = -read_f32_checked(ctrl + CTRL_FORWARD[0]).ok()?;
    let z = -read_f32_checked(ctrl + CTRL_FORWARD[2]).ok()?;
    let length = (x * x + z * z).sqrt();
    (0.5..1.5).contains(&length).then(|| [x / length, z / length])
}

/// The chest the player stands in front of and faces most directly, if any.
pub fn chest_in_front(here: [f32; 3], forward: [f32; 2], chests: &[[f32; 3]]) -> Option<usize> {
    chests
        .iter()
        .enumerate()
        .filter_map(|(index, chest)| {
            let (dx, dz) = (chest[0] - here[0], chest[2] - here[2]);
            let distance = (dx * dx + dz * dz).sqrt();
            let facing = if distance > 0.01 { (dx * forward[0] + dz * forward[1]) / distance } else { 1.0 };
            (distance <= CHEST_REACH_METRES && facing >= CHEST_FACING_COSINE).then_some((index, facing))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(index, _)| index)
}

/// A random good weapon (fully upgraded) or ring, as an item id.
fn pick_prize(kind: PrizeKind) -> Result<(ItemId, String), String> {
    let weapon = match kind {
        PrizeKind::Weapon => true,
        PrizeKind::Ring => false,
        PrizeKind::Either => random_index(2) == 0,
    };
    if !weapon {
        let ring = PRIZE_RINGS[random_index(PRIZE_RINGS.len())];
        let item = ItemId::new(ItemCategory::Accessory, ring).map_err(|error| format!("ring {ring}: {error:?}"))?;
        return Ok((item, format!("ring {ring}")));
    }
    let (ashes, ringed_city) = unsafe { darksouls3::cs::CSDlc::instance() }
        .map(|dlc| (dlc.dlc1_installed, dlc.dlc2_installed))
        .unwrap_or((false, false));
    let mut pool = PRIZE_WEAPONS.to_vec();
    if ashes {
        pool.extend(ASHES_OF_ARIANDEL_PRIZE_WEAPONS);
    }
    if ringed_city {
        pool.extend(RINGED_CITY_PRIZE_WEAPONS);
    }
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the weapon list is unavailable: {error:?}"))?;
    let weapons = regulation.get_param::<EQUIP_PARAM_WEAPON_ST>();
    pool.retain(|base| weapons.get(u64::from(*base)).is_some());
    if pool.is_empty() {
        return Err("none of the prize weapons are in the game data".to_string());
    }
    let base = pool[random_index(pool.len())];
    let level = weapons.get(u64::from(base)).map_or(0, |row| max_weapon_level(regulation, row));
    let id = base + level;
    let item = ItemId::new(ItemCategory::Weapon, id).map_err(|error| format!("weapon {id}: {error:?}"))?;
    Ok((item, format!("weapon {id} (+{level})")))
}

fn give_prize(kind: PrizeKind) -> Result<String, String> {
    let (item, label) = pick_prize(kind)?;
    let game_data = unsafe { GameDataMan::instance_mut() }
        .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
    game_data.give_item_directly(item, 1);
    if let Ok(menu) = unsafe { darksouls3::sprj::ItemGetMenuMan::instance_mut() } {
        menu.show_item(item, 1, false);
    }
    Ok(label)
}

/// The effect a chest gets before it goes, by the game's own name "City Map Sniper
/// Transparent effect". Of those tried in game it fades out best; 10633 faded out and
/// back in, and 10635 stopped at about 20%.
const CHEST_VANISH_EFFECT: i32 = 12_030;
const CHEST_VANISH_SECONDS: f32 = 1.5;

fn remove_chest(chr: usize) {
    vanish(chr, CHEST_VANISH_EFFECT, CHEST_VANISH_SECONDS);
}

/// Spawned characters fading out: character, frames until it is removed.
static VANISHING: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

/// Puts `effect` on a spawned character, then removes it `seconds` later. With effect 0 it
/// is removed straight away.
fn vanish(chr: usize, effect: i32, seconds: f32) {
    if effect > 0 {
        if let Err(error) = add_sp_effect(chr, effect) {
            crate::log::log(format!("vanish: effect {effect} failed: {error}"));
        }
    }
    let frames = (seconds.max(0.0) * 60.0) as u32;
    let mut list = VANISHING.lock().unwrap_or_else(|poison| poison.into_inner());
    if !list.iter().any(|(known, _)| *known == chr) {
        list.push((chr, frames));
    }
}

pub fn poll_vanishing() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut list = VANISHING.lock().unwrap_or_else(|poison| poison.into_inner());
        if list.is_empty() {
            return;
        }
        let present = spawned_chrs();
        list.retain(|(chr, _)| present.contains(chr));
        let mut due = Vec::new();
        for (chr, frames) in list.iter_mut() {
            if *frames == 0 {
                due.push(*chr);
            } else {
                *frames -= 1;
            }
        }
        list.retain(|(chr, _)| !due.contains(chr));
        drop(list);
        if !due.is_empty() {
            let mut requested = DELETE_REQUESTED.lock().unwrap_or_else(|poison| poison.into_inner());
            if let Err(error) = delete_spawns(&due, &mut requested) {
                crate::log::log(format!("vanish: could not remove: {error}"));
            }
        }
    }));
}

/// Developer test: the closest spawned character gets `effect` and is removed `seconds`
/// later, to see which effect makes a good way out.
pub fn try_vanish_effect(effect: i32, seconds: f32) -> Result<String, String> {
    add_sp_effect_routine()?;
    let spawned = spawned_chrs();
    let (chr, distance) = *chrs_near_player(NEARBY_METRES)
        .iter()
        .find(|(chr, _)| spawned.contains(chr))
        .ok_or_else(|| format!("no spawned enemy is within {NEARBY_METRES} m"))?;
    vanish(chr, effect, seconds);
    Ok(format!("{chr:#x} ({distance:.1} m away) gets effect {effect} and goes in {seconds} seconds"))
}

/// What happens when chest `picked` is opened.
fn open_chest(game: &mut ChestGame, picked: usize) {
    let chests = game.chests.clone();
    if picked == game.winner {
        for chr in &chests {
            remove_chest(*chr);
        }
        match give_prize(game.prize) {
            Ok(label) => crate::log::log(format!("Mimic Chests: the right chest, the prize is {label}")),
            Err(error) => crate::log::log(format!("Mimic Chests: the prize could not be given: {error}")),
        }
    } else {
        crate::log::log("Mimic Chests: a Mimic".to_string());
        remove_chest(chests[game.winner]);
        crate::input::block_input(BITE_LOCK_SECONDS);
        for (index, chr) in chests.iter().enumerate() {
            if index == game.winner {
                continue;
            }
            let follow = if index == picked { MIMIC_BITE } else { MIMIC_STAND };
            if let Err(error) = request_animation(*chr, MIMIC_OPEN) {
                crate::log::log(format!("Mimic Chests: could not wake a Mimic: {error}"));
            }
            game.later.push((*chr, follow, MIMIC_FOLLOW_FRAMES));
            let wake_after = if index == picked { AFTER_BITE_FRAMES } else { MIMIC_FOLLOW_FRAMES + 1 };
            game.later.push((*chr, WAKE, wake_after));
            game.later.push((*chr, LOG_EFFECTS, wake_after + 30));
            game.later.push((*chr, LOG_EFFECTS, wake_after + 150));
        }
    }
    game.phase = ChestPhase::Done;
    game.done_at = Some(Instant::now());
}

/// Time is up, or a chest was attacked: every chest turns out to be a Mimic, and there is
/// no prize. One that was hit is awake already and only gets the wake-up effect.
fn wake_all_chests(game: &mut ChestGame, hit: Option<usize>) {
    let reason = if hit.is_some() { "a chest was attacked" } else { "time is up" };
    crate::log::log(format!("Mimic Chests: {reason}, all three wake"));
    for (index, chr) in game.chests.clone().into_iter().enumerate() {
        if hit == Some(index) {
            game.later.push((chr, WAKE, 1));
            continue;
        }
        let _ = request_animation(chr, MIMIC_OPEN);
        game.later.push((chr, MIMIC_STAND, MIMIC_FOLLOW_FRAMES));
        game.later.push((chr, WAKE, MIMIC_FOLLOW_FRAMES + 1));
    }
    game.phase = ChestPhase::Done;
    game.done_at = Some(Instant::now());
}

pub fn poll_mimic_chests() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = CHEST_GAME.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(game) = slot.as_mut() else {
            return;
        };
        if load_screen_open() {
            *slot = None;
            return;
        }
        // Animations asked for a moment after the first.
        let present = spawned_chrs();
        for entry in game.later.iter_mut() {
            entry.2 = entry.2.saturating_sub(1);
            if entry.2 == 0 && present.contains(&entry.0) {
                let result = if entry.1 == LOG_EFFECTS {
                    crate::log::log(format!("Mimic Chests: {:#x} effects {:?}", entry.0, chr_sp_effects(entry.0)));
                    Ok(())
                } else if entry.1 == WAKE {
                    let woken = wake_mimic(entry.0);
                    crate::log::log(format!("Mimic Chests: {:#x} woken, effects now {:?}", entry.0, chr_sp_effects(entry.0)));
                    woken
                } else {
                    request_animation(entry.0, entry.1)
                };
                if let Err(error) = result {
                    crate::log::log(format!("Mimic Chests: step {} failed: {error}", entry.1));
                }
            }
        }
        game.later.retain(|entry| entry.2 > 0);

        match game.phase {
            ChestPhase::Spawning => {
                let made: Vec<usize> = std::mem::take(&mut *NEW_MIMICS.lock().unwrap_or_else(|poison| poison.into_inner()));
                for chr in made {
                    if !game.chests.contains(&chr) && game.chests.len() < MIMIC_ROW.len() {
                        game.chests.push(chr);
                    }
                }
                if game.chests.len() >= MIMIC_ROW.len() {
                    game.full_health = game.chests.iter().map(|chr| chr_health(*chr).unwrap_or(0)).collect();
                    game.phase = ChestPhase::Waiting;
                    game.started = Instant::now();
                    crate::log::log(format!("Mimic Chests: chests ready, the prize is in chest {}", game.winner + 1));
                    for chr in &game.chests {
                        crate::log::log(format!("Mimic Chests: {chr:#x} effects {:?}", chr_sp_effects(*chr)));
                    }
                } else if game.started.elapsed() > Duration::from_secs(CHEST_SPAWN_SECONDS) {
                    crate::log::log("Mimic Chests: the chests never appeared".to_string());
                    *slot = None;
                }
            }
            ChestPhase::Waiting => {
                if game.chests.iter().any(|chr| !present.contains(chr)) {
                    crate::log::log("Mimic Chests: a chest is gone, ending".to_string());
                            *slot = None;
                    return;
                }
                if game.place_frames < MIMIC_PLACE_FRAMES {
                    game.place_frames += 1;
                    place_row(&game.chests, &game.spots);
                }
                // Hitting a chest picks it too.
                let hit = game
                    .chests
                    .iter()
                    .zip(&game.full_health)
                    .position(|(chr, full)| chr_health(*chr).is_some_and(|hp| hp < *full));
                // Attacking is not a way to pick: all three wake and nobody wins.
                if let Some(index) = hit {
                    wake_all_chests(game, Some(index));
                    return;
                }
                let down = interact_down();
                let pressed = down && !game.interact_was_down;
                game.interact_was_down = down;
                if pressed {
                    let Ok(player) = local_player() else {
                        return;
                    };
                    let player = player as usize;
                    let spots: Vec<[f32; 3]> = game.chests.iter().filter_map(|chr| chr_position(*chr)).collect();
                    if let (Some(here), Some(forward)) = (chr_position(player), player_forward(player)) {
                        if spots.len() == game.chests.len() {
                            if let Some(index) = chest_in_front(here, forward, &spots) {
                                open_chest(game, index);
                                return;
                            }
                            let nearest = spots
                                .iter()
                                .map(|spot| distance_between(here, *spot))
                                .fold(f32::MAX, f32::min);
                            crate::log::log(format!(
                                "Mimic Chests: interact pressed, but no chest straight ahead (closest {nearest:.1} m)"
                            ));
                        }
                    }
                }
                if game.started.elapsed() > Duration::from_secs(game.seconds) {
                    wake_all_chests(game, None);
                }
            }
            ChestPhase::Done => {
                let finished = game.later.is_empty()
                    && game.done_at.is_some_and(|at| at.elapsed() > Duration::from_secs(CHEST_AFTERMATH_SECONDS));
                if finished {
                            *slot = None;
                }
            }
        }
    }));
}

const GEAR_SNAPSHOT_FILE: &str = "gear-snapshots.json";
/// Seconds between gear snapshots.
pub const GEAR_SNAPSHOT_EVERY: u64 = 300;

/// Weapons and armor equipped at one moment, by item id. 0 or a fist or bare id means empty.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct GearSnapshot {
    pub taken: u64,
    pub character: String,
    pub weapons: [u32; 6],
    pub armor: [u32; 4],
}

/// Two snapshots that take turns, so a bad moment never overwrites both.
#[derive(Default, Deserialize, Serialize)]
pub struct GearSnapshots {
    pub slots: [Option<GearSnapshot>; 2],
    pub next: usize,
}

impl GearSnapshots {
    pub fn store(&mut self, snapshot: GearSnapshot) {
        let slot = self.next % self.slots.len();
        self.slots[slot] = Some(snapshot);
        self.next = (slot + 1) % self.slots.len();
    }
}

pub fn gear_snapshot_due(now: u64, last: Option<u64>) -> bool {
    last.is_none_or(|last| now >= last + GEAR_SNAPSHOT_EVERY)
}

/// When this session last took a snapshot. The first one comes as soon as it safely can.
static LAST_GEAR_SNAPSHOT: Mutex<Option<u64>> = Mutex::new(None);

fn gear_snapshot_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::log::app_dir()?.join(GEAR_SNAPSHOT_FILE))
}

fn load_gear_snapshots() -> GearSnapshots {
    gear_snapshot_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_gear_snapshots(snapshots: &GearSnapshots) -> Result<(), String> {
    let path = gear_snapshot_path()?;
    let text = serde_json::to_string_pretty(snapshots).map_err(|error| error.to_string())?;
    save_in_background(FileJob::Replace { what: "gear snapshot", path, text });
    Ok(())
}

fn equipped_item(player: &PlayerIns, slot: u32) -> u32 {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment };
    u32::try_from(equipment.equipment_indexes[slot as usize])
        .ok()
        .and_then(|index| equipment.equip_inventory_data.items_data[index].as_option())
        .map_or(0, |entry| entry.item_id.param_id())
}

/// Only while nothing has swapped gear temporarily, so a snapshot always holds the player's own.
fn gear_snapshot_allowed(player: &PlayerIns) -> bool {
    let restores = pending();
    restores.armor.is_none()
        && restores.gear_event().is_none()
        && player.super_chr_ins.modules.data.hp > 0
        && !load_screen_open()
        && FAKE_DEATH.lock().unwrap_or_else(|poison| poison.into_inner()).is_none()
}

pub fn poll_gear_snapshots() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let now = unix_now();
        let mut last = LAST_GEAR_SNAPSHOT.lock().unwrap_or_else(|poison| poison.into_inner());
        if !gear_snapshot_due(now, *last) {
            return;
        }
        let Ok(player) = local_player() else {
            return;
        };
        let player = unsafe { &*player };
        let (loaded, character) = read_character();
        if !loaded || !gear_snapshot_allowed(player) {
            return;
        }
        *last = Some(now);
        let snapshot = GearSnapshot {
            taken: now,
            character,
            weapons: WEAPON_SLOTS.map(|slot| equipped_item(player, slot)),
            armor: ARMOR_SLOTS.map(|slot| equipped_item(player, slot)),
        };
        let mut snapshots = load_gear_snapshots();
        snapshots.store(snapshot);
        if let Err(error) = save_gear_snapshots(&snapshots) {
            crate::log::log(format!("gear snapshot could not be saved: {error}"));
        }
    }));
}

/// Puts one slot back. Returns whether it changed, and whether an item had to be given back.
fn restore_gear_slot(
    player: &mut PlayerIns,
    category: ItemCategory,
    slot: u32,
    item: u32,
    real: bool,
) -> Result<(bool, bool), String> {
    if equipped_item(player, slot) == item {
        return Ok((false, false));
    }
    if !real {
        call_equip(player, slot, -1)?;
        return Ok((true, false));
    }
    let mut given = false;
    let index = match current_index(player, category, item, -1) {
        Some(index) => index,
        None => {
            let id = ItemId::new(category, item).map_err(|error| format!("item {item} is invalid: {error:?}"))?;
            let game_data = unsafe { GameDataMan::instance_mut() }
                .map_err(|error| format!("GameDataMan is unavailable: {error:?}"))?;
            game_data.give_item_directly(id, 1);
            given = true;
            current_index(player, category, item, -1).ok_or_else(|| format!("item {item} could not be given back"))?
        }
    };
    call_equip(player, slot, index)?;
    Ok((true, given))
}

pub fn restore_gear(slot: usize) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    // One lock for both checks: taking it twice in one expression would deadlock.
    let swapping = {
        let restores = pending();
        restores.armor.is_some() || restores.gear_event().is_some()
    };
    if swapping {
        return Err("a temporary armor, Bombardier, Firestarter or Ooga Booga swap is still active".to_string());
    }
    let snapshots = load_gear_snapshots();
    let snapshot = snapshots
        .slots
        .get(slot)
        .and_then(Option::as_ref)
        .ok_or_else(|| format!("gear snapshot {} is empty", slot + 1))?;
    if snapshot.character != read_character().1 {
        return Err(format!("gear snapshot {} belongs to {}", slot + 1, snapshot.character));
    }
    let mut changed = 0;
    let mut given = 0;
    for (weapon_slot, weapon) in WEAPON_SLOTS.into_iter().zip(snapshot.weapons) {
        let (did_change, did_give) =
            restore_gear_slot(player, ItemCategory::Weapon, weapon_slot, weapon, is_removable_weapon(weapon))?;
        changed += usize::from(did_change);
        given += usize::from(did_give);
    }
    for (armor_slot, piece) in ARMOR_SLOTS.into_iter().zip(snapshot.armor) {
        let (did_change, did_give) =
            restore_gear_slot(player, ItemCategory::Protector, armor_slot, piece, is_real_armor(piece))?;
        changed += usize::from(did_change);
        given += usize::from(did_give);
    }
    Ok(format!("gear snapshot {} restored: {changed} slots changed, {given} items given back", slot + 1))
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
            slot.remember_ended(EndedKind::Stats(active));
        } else {
            crate::log::log("stat shuffle not restored: the stat total changed");
        }
        save_pending(&slot);
    }));
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Right,
    Left,
    Either,
}

impl Hand {
    pub fn from_index(index: i64) -> Self {
        match index {
            1 => Self::Left,
            2 => Self::Either,
            _ => Self::Right,
        }
    }
}

/// EquipGameData: which of a hand's three weapons is in use, 0 to 2, for the left and
/// right hand. The weapon slots go left 1, right 1, left 2, right 2, left 3, right 3.
const ACTIVE_LEFT_WEAPON_OFFSET: usize = 0x14;
const ACTIVE_RIGHT_WEAPON_OFFSET: usize = 0x18;

/// The slot of the weapon a hand holds right now. If the game's choice cannot be read,
/// the first slot of that hand with a real weapon.
fn held_weapon_slot(player: &PlayerIns, right: bool) -> Option<(u32, i32, u32)> {
    let equipment = unsafe { &player.player_game_data.as_ref().equipment } as *const _ as usize;
    let offset = if right { ACTIVE_RIGHT_WEAPON_OFFSET } else { ACTIVE_LEFT_WEAPON_OFFSET };
    let first = u32::from(right);
    let active = read_u32_checked(equipment + offset).ok().filter(|active| *active <= 2);
    let slot = match active {
        Some(active) => first + active * 2,
        None => (0..3)
            .map(|step| first + step * 2)
            .find(|slot| equipped_weapon(player, *slot).is_some_and(|(_, weapon)| is_removable_weapon(weapon)))?,
    };
    equipped_weapon(player, slot)
        .filter(|(_, weapon)| is_removable_weapon(*weapon))
        .map(|(index, weapon)| (slot, index, weapon))
}

/// The held weapon of the chosen hand, or of a random hand that holds one.
fn pick_held_weapon(player: &PlayerIns, hand: Hand) -> Result<(u32, i32, u32), String> {
    let candidates: Vec<(u32, i32, u32)> = match hand {
        Hand::Right => held_weapon_slot(player, true).into_iter().collect(),
        Hand::Left => held_weapon_slot(player, false).into_iter().collect(),
        Hand::Either => [true, false].into_iter().filter_map(|right| held_weapon_slot(player, right)).collect(),
    };
    if candidates.is_empty() {
        return Err(match hand {
            Hand::Right => "the right hand holds no weapon",
            Hand::Left => "the left hand holds no weapon",
            Hand::Either => "neither hand holds a weapon",
        }
        .to_string());
    }
    Ok(candidates[random_index(candidates.len())])
}

fn weapon_event_player() -> Result<&'static mut PlayerIns, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    // The swaps take care of the weapons themselves and put them back later.
    if let Some(active) = pending().gear_event() {
        return Err(active.to_string());
    }
    Ok(player)
}

fn hand_name(slot: u32) -> &'static str {
    if slot % 2 == 1 {
        "right"
    } else {
        "left"
    }
}

/// Takes the held weapon out of the hand. It stays in the inventory.
pub fn unequip_weapon(hand: Hand) -> Result<String, String> {
    let player = weapon_event_player()?;
    let (slot, _, weapon) = pick_held_weapon(player, hand)?;
    call_equip(player, slot, -1)?;
    Ok(format!("unequipped weapon {weapon} from the {} hand", hand_name(slot)))
}

/// Takes the held weapon out of the hand and deletes it from the inventory for good.
/// A gear snapshot on the General page can bring it back.
pub fn delete_weapon(hand: Hand) -> Result<String, String> {
    let player = weapon_event_player()?;
    let (slot, index, weapon) = pick_held_weapon(player, hand)?;
    call_equip(player, slot, -1)?;
    if !remove_inventory_entry(player, index)? {
        return Err(format!("weapon {weapon} was unequipped, but the game kept it in the inventory"));
    }
    Ok(format!("deleted weapon {weapon} from the {} hand", hand_name(slot)))
}

pub fn random_weapon(options: WeaponOptions) -> Result<String, String> {
    let player = unsafe { &mut *local_player()? };
    if player.super_chr_ins.modules.data.hp <= 0 {
        return Err("the local player is dead".to_string());
    }
    if let Some(active) = pending().gear_event() {
        return Err(active.to_string());
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

/// WorldChrMan: two more character sets next to the spawned one. Capacity, then entries.
const OTHER_CHR_SETS: [usize; 2] = [0x38, 0x50];

fn chr_set_members(set: usize) -> Vec<usize> {
    let capacity = read_u32_checked(set).unwrap_or(0) as usize;
    let entries = read_usize_checked(set + 8).unwrap_or(0);
    if capacity == 0 || capacity > MAX_CHRS_PER_BLOCK as usize || entries < 0x10_000 {
        return Vec::new();
    }
    (0..capacity)
        .filter_map(|index| read_usize_checked(entries + index * CHR_SET_ENTRY_SIZE).ok())
        .filter(|chr| *chr >= 0x10_000)
        .collect()
}

fn dump_region(text: &mut String, name: &str, address: usize, size: usize) {
    let mut bytes = vec![0u8; size];
    let got = if address >= 0x10_000 { copy_readable(address, &mut bytes) } else { 0 };
    text.push_str(&format!("REGION {name} {address:#x} {got:#x}\n"));
    for (index, chunk) in bytes[..got].chunks(16).enumerate() {
        text.push_str(&format!("{:04X} ", index * 16));
        for byte in chunk {
            text.push_str(&format!("{byte:02X} "));
        }
        text.push('\n');
    }
}

/// Writes the state of every player-type character in the area to a file: the player,
/// real NPC phantoms the game placed, and the ones this mod spawned. For comparing a
/// working summon with ours.
pub fn dump_phantoms() -> Result<String, String> {
    let player = local_player()? as usize;
    let player_class = read_usize_checked(player)?;
    let world = unsafe { WorldChrMan::instance() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?;
    let mut found: Vec<(String, usize)> = Vec::new();
    let slots = world.world_block_chr.as_ptr() as usize;
    let slot_size = std::mem::size_of_val(&world.world_block_chr) / world.world_block_chr.len();
    for index in 0..world.world_block_chr.len() {
        let block = slots + index * slot_size;
        let capacity = read_u32_checked(block + BLOCK_CHR_SET_CAPACITY).unwrap_or(0) as usize;
        let entries = read_usize_checked(block + BLOCK_CHR_SET_ENTRIES).unwrap_or(0);
        if capacity == 0 || capacity > MAX_CHRS_PER_BLOCK as usize || entries < 0x10_000 {
            continue;
        }
        for entry in 0..capacity {
            let chr = read_usize_checked(entries + entry * CHR_SET_ENTRY_SIZE).unwrap_or(0);
            if chr >= 0x10_000 {
                found.push((format!("block{index}[{entry}]"), chr));
            }
        }
    }
    let world = world as *const WorldChrMan as usize;
    for chr in chr_set_members(world + DEBUG_CHR_SET) {
        found.push(("spawned".to_string(), chr));
    }
    for (index, offset) in OTHER_CHR_SETS.iter().enumerate() {
        for chr in chr_set_members(world + offset) {
            found.push((format!("set{index}"), chr));
        }
    }
    found.push(("player".to_string(), player));
    found.sort_by_key(|(_, chr)| *chr);
    found.dedup_by_key(|(_, chr)| *chr);
    // Regular enemies too, the ones near the player, closest first: what they see and who
    // they are after, next to the phantoms.
    let position = |chr: usize| -> Option<[f32; 3]> {
        let physics = module_pointer(chr, PHYSICS_MODULE_OFFSET).ok()?;
        let mut bytes = [0u8; 12];
        (copy_readable(physics + PHYSICS_POSITION, &mut bytes) == 12).then(|| {
            [0, 1, 2].map(|axis| f32::from_ne_bytes(bytes[axis * 4..axis * 4 + 4].try_into().unwrap()))
        })
    };
    let here = position(player);
    let distance = |chr: usize| -> Option<f32> {
        let (a, b) = (here?, position(chr)?);
        Some(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt())
    };
    let mut enemies: Vec<(String, usize, f32)> = found
        .iter()
        .filter(|(_, chr)| read_usize_checked(*chr).is_ok_and(|class| class != player_class))
        .filter_map(|(origin, chr)| distance(*chr).map(|metres| (origin.clone(), *chr, metres)))
        .filter(|(_, _, metres)| *metres <= DUMP_ENEMY_RANGE)
        .collect();
    enemies.sort_by(|a, b| a.2.total_cmp(&b.2));
    enemies.truncate(DUMP_ENEMIES);
    found.retain(|(_, chr)| read_usize_checked(*chr) == Ok(player_class));

    let mut text = String::new();
    if let Some((session, _)) = phantom_list() {
        text.push_str(&format!("SESSION {session:#x}\n"));
        dump_region(&mut text, "session", session, 0x200);
    }
    for (origin, chr) in &found {
        let chr = *chr;
        let pointer = |offset: usize| read_usize_checked(chr + offset).unwrap_or(0);
        let ctrl = pointer(CHR_CTRL_OFFSET);
        let modules = pointer(MODULES_OFFSET);
        let module = |offset: usize| read_usize_checked(modules + offset).unwrap_or(0);
        let player_data = pointer(PLAYER_GAME_DATA);
        text.push_str(&format!(
            "CHR {chr:#x} origin={origin} name={:?} npc={} type={} team={} hp={:?}\n",
            chr_name(chr),
            read_u32_checked(chr + 0x68).unwrap_or(0),
            read_u32_checked(chr + CHR_TYPE_OFFSET).unwrap_or(0),
            read_u32_checked(chr + CHR_TEAM_OFFSET).unwrap_or(0),
            chr_health(chr),
        ));
        dump_region(&mut text, "chr_head", chr, 0x200);
        dump_region(&mut text, "chr_flags", chr + 0x1A00, 0x180);
        dump_region(&mut text, "chr_tail", chr + 0x1EC0, 0x2E0);
        text.push_str(&format!("NOTE {}\n", team_report(chr)));
        text.push_str(&format!("NOTE {}\n", ai_target_list(chr, false)));
        dump_region(&mut text, "ctrl", ctrl, 0x200);
        dump_region(&mut text, "ctrl_sub48", read_usize_checked(ctrl + 0x48).unwrap_or(0), 0x140);
        dump_region(&mut text, "modules", modules, 0x90);
        dump_region(&mut text, "data_module", module(DATA_MODULE_OFFSET), 0x140);
        dump_region(&mut text, "event_module", module(EVENT_MODULE_OFFSET), 0x80);
        dump_region(&mut text, "physics_module", module(PHYSICS_MODULE_OFFSET), 0x100);
        dump_region(&mut text, "player_data", player_data, 0x140);
        dump_region(&mut text, "ai", pointer(0x1FA8), 0x200);
        dump_region(&mut text, "ai2", pointer(0x1FB0), 0x100);
        dump_region(&mut text, "npc_lookup", pointer(NPC_ROW_LOOKUP), 0x10);
        dump_region(&mut text, "ai_module", module(AI_MODULE_OFFSET), 0x200);
    }
    for (origin, chr, metres) in &enemies {
        let chr = *chr;
        let pointer = |offset: usize| read_usize_checked(chr + offset).unwrap_or(0);
        let modules = pointer(MODULES_OFFSET);
        let module = |offset: usize| read_usize_checked(modules + offset).unwrap_or(0);
        text.push_str(&format!(
            "CHR {chr:#x} origin={origin} name={:?} npc={} type={} team={} hp={:?} kind=enemy distance={metres:.1}\n",
            chr_name(chr),
            read_u32_checked(chr + 0x68).unwrap_or(0),
            read_u32_checked(chr + CHR_TYPE_OFFSET).unwrap_or(0),
            read_u32_checked(chr + CHR_TEAM_OFFSET).unwrap_or(0),
            chr_health(chr),
        ));
        dump_region(&mut text, "chr_head", chr, 0x200);
        dump_region(&mut text, "chr_flags", chr + 0x1A00, 0x180);
        dump_region(&mut text, "chr_tail", chr + 0x1EC0, 0x150);
        text.push_str(&format!("NOTE {}\n", team_report(chr)));
        text.push_str(&format!("NOTE {}\n", ai_target_list(chr, false)));
        dump_region(&mut text, "modules", modules, 0x90);
        dump_region(&mut text, "data_module", module(DATA_MODULE_OFFSET), 0x140);
        dump_region(&mut text, "physics_module", module(PHYSICS_MODULE_OFFSET), 0x100);
        dump_region(&mut text, "ai_module", module(AI_MODULE_OFFSET), 0x200);
    }
    let dir = crate::log::app_dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("phantom-dump-{stamp}.txt"));
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(format!("recorded {} player-type characters and {} enemies nearby in {}", found.len(), enemies.len(), path.display()))
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

pub(crate) fn copy_readable(addr: usize, dest: &mut [u8]) -> usize {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if dest.is_empty() || addr < 0x10_000 || addr > isize::MAX as usize - dest.len() {
        return 0;
    }
    if read_into(addr, dest) {
        return dest.len();
    }
    // Only part of it may be readable: ask about the region and copy what is there.
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

/// Copies from the game's memory, or fails without crashing when the address is not
/// readable. Asking Windows about the memory region first (VirtualQuery) is what the mod
/// did before, but Windows answers it by walking every page of the region, and in the
/// game's large heaps that took up to half a millisecond per read. A read through
/// ReadProcessMemory fails the same way on a bad address and costs a few microseconds.
fn read_into(addr: usize, dest: &mut [u8]) -> bool {
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::GetCurrentProcess;
    if dest.is_empty() || addr < 0x10_000 || addr > isize::MAX as usize - dest.len() {
        return false;
    }
    let mut read = 0usize;
    let copied = unsafe {
        ReadProcessMemory(GetCurrentProcess(), addr as *const _, dest.as_mut_ptr().cast(), dest.len(), Some(&mut read))
    };
    copied.is_ok() && read == dest.len()
}

/// What one VirtualQuery and one ReadProcessMemory on the player cost here, in microseconds,
/// for the performance report.
pub fn memory_check_cost() -> Option<(f64, f64)> {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION};
    const ROUNDS: u32 = 64;
    let player = local_player().ok()? as usize;
    let start = Instant::now();
    for _ in 0..ROUNDS {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        unsafe { VirtualQuery(Some(player as *const _), &mut info, std::mem::size_of::<MEMORY_BASIC_INFORMATION>()) };
        std::hint::black_box(info.RegionSize);
    }
    let query = start.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS);
    let start = Instant::now();
    for _ in 0..ROUNDS {
        let mut bytes = [0u8; 8];
        std::hint::black_box(read_into(player, &mut bytes));
    }
    let read = start.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS);
    Some((query, read))
}

fn readable_qword(addr: usize) -> bool {
    let mut bytes = [0u8; 8];
    read_into(addr, &mut bytes)
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
    /// A human NPC invader. It is built as a player-type character, not through the debug
    /// creator, and its CharaInit row has the same id as its NPC row.
    invader: bool,
    /// Key of an invader's note: what to announce when it arrives. 0 for none.
    label: u32,
    /// A human NPC that arrives as a white phantom on the player's side.
    friendly: bool,
}

/// Named human invaders. Each has an NPC row and a CharaInit row (gear, stats, face) with
/// the same id.
const BASE_INVADERS: [(u32, &str); 10] = [
    (27_420, "Yellowfinger Heysel"),
    (27_700, "Holy Knight Hodrick"),
    (27_800, "Pale Shade of Londor"),
    (27_920, "Lion Knight Albert"),
    (28_040, "Daughter of Crystal Kriemhild"),
    (28_700, "Havel the Rock"),
    (29_200, "Knight Slayer Tsorig"),
    (29_340, "Creighton of Mirrah"),
    (29_440, "Alva, Seeker of the Spurned"),
    (29_540, "Longfinger Kirk"),
];
const ASHES_OF_ARIANDEL_INVADERS: [(u32, &str); 1] = [(21_140, "Sir Vilhelm")];
const RINGED_CITY_INVADERS: [(u32, &str); 1] = [(22_340, "Moaning Knight")];

pub fn known_invader_name(id: u32) -> Option<&'static str> {
    BASE_INVADERS
        .iter()
        .chain(ASHES_OF_ARIANDEL_INVADERS.iter())
        .chain(RINGED_CITY_INVADERS.iter())
        .find(|(npc_id, _)| *npc_id == id)
        .map(|(_, name)| *name)
}

pub fn parse_npc_param_ids(text: &str) -> Vec<u32> {
    text.split(|ch: char| ch == ',' || ch.is_whitespace())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

/// Built-in invaders when `requested` is empty, otherwise only those ids that exist in `present`.
pub fn choose_invader_ids(requested: &[u32], present: &[u32]) -> Vec<u32> {
    let pool: Vec<u32> = if requested.is_empty() {
        present.to_vec()
    } else {
        requested.iter().copied().filter(|id| present.binary_search(id).is_ok()).collect()
    };
    pool
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
/// TGA: character kind. 2 is Dark Spirit (red phantom look and death mist).
const CHR_TYPE_OFFSET: usize = 0x70;
const CHR_TYPE_DARK_SPIRIT: i32 = 2;
const CHR_TYPE_WHITE_PHANTOM: i32 = 1;
/// TGA: team. 16 is Dark Spirit (hostile to the host).
const CHR_TEAM_OFFSET: usize = 0x74;
const CHR_TEAM_DARK_SPIRIT: u32 = 16;
const CHR_TEAM_WHITE_PHANTOM: u32 = 2;
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
    npc_id: u32,
}

/// Mimics the queue has just made, for Mimic Chests to pick up as its chests.
static NEW_MIMICS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

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
    /// The model check below is for enemy objects. A player-type NPC keeps its body
    /// elsewhere, so it is never treated as frozen.
    skip_frozen_delete: bool,
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
            // Spawned enemies go on the AI's target list too, so allies and invaders see
            // them. Allies and invaders get there in their own setup a little later.
            SpawnState::Fine if !check.skip_frozen_delete => {
                let mut listed = AI_LISTED_NPCS.lock().unwrap_or_else(|poison| poison.into_inner());
                if !listed.contains(&check.chr) {
                    listed.push(check.chr);
                }
            }
            SpawnState::Fine => {}
            SpawnState::Gone => failed += 1,
            SpawnState::Frozen if check.skip_frozen_delete => {}
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
        forget_invader_name(chr);
        if !requested.contains(&chr) {
            let handle = read_u32_checked(chr + 0x08).ok();
            call(world, chr);
            requested.push(chr);
            // An ally or invader also has a place in the session's phantom list. Left
            // there, it would describe whoever is spawned into the same slot next.
            if let Some(handle) = handle {
                forget_phantom_entry(handle);
            }
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
                invader: false,
                friendly: false,
                label: 0,
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
    queue.in_flight = Some(EnemyInFlight { previous, frames: 0, npc_id: enemy.npc_id });
    Ok(())
}

/// The Mimic of the High Wall. Every area's Mimic shares the c2120 model.
const MIMIC: u32 = 212_000;
/// Where the three chests stand, in metres to the side of straight ahead.
const MIMIC_ROW: [f32; 3] = [-1.8, 0.0, 1.8];
const MIMIC_DISTANCE: f32 = 3.0;
/// Positions are written for this many frames, in case the game's physics puts a Mimic
/// back on the first one.
const MIMIC_PLACE_FRAMES: u32 = 10;
/// A Mimic's wake-up when it is hit.
const MIMIC_WAKE_ANIMATION: i32 = 2_003_010;
const MIMIC_TEST_SECONDS: u64 = 90;

struct MimicWatch {
    started: Instant,
    last_log: Instant,
    /// Spawned characters that were there before, so the new chests can be told apart.
    before: Vec<usize>,
    last_player_anim: i32,
    last_anims: Vec<(usize, i32)>,
    /// Where each new Mimic goes and which way it faces, worked out from where the player
    /// stood when the test started.
    spots: Vec<([f32; 3], f32)>,
    placed: Vec<usize>,
    place_frames: u32,
    /// The first Mimic hit; the others are then told to wake up.
    hit: Option<usize>,
}

static MIMIC_WATCH: Mutex<Option<MimicWatch>> = Mutex::new(None);

/// Developer test for the chest game: three Mimics side by side in front of you. For a
/// while afterwards the log shows your animation, and each Mimic's animation and distance,
/// so it shows whether they stay asleep and what opening one looks like.
pub fn test_mimic_chests(spawn: bool, npc_id: u32, think_override: u32) -> Result<String, String> {
    log_mimic_throws();
    if !spawn {
        start_mimic_watch(Vec::new())?;
        return Ok(format!(
            "logging animations of everything within {NEARBY_METRES} m for {MIMIC_TEST_SECONDS} seconds"
        ));
    }
    let routine = create_npc_address()?;
    let creator = debug_creator()?;
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the NPC list is unavailable: {error:?}"))?;
    if regulation.get_param::<NPC_PARAM_ST>().get(u64::from(npc_id)).is_none() {
        return Err(format!("NPC {npc_id} is missing from the game data"));
    }
    let think_ids: Vec<u32> = regulation
        .get_param::<NPC_THINK_PARAM_ST>()
        .iter()
        .filter(|(id, _)| *id > 0)
        .map(|(id, _)| id as u32)
        .collect();
    let think_id = if think_override > 0 {
        think_override
    } else {
        think_candidates(npc_id, &think_ids).into_iter().next().ok_or("this NPC has no AI row")?
    };
    let picks: Vec<EnemyChoice> = MIMIC_ROW
        .iter()
        .map(|_| EnemyChoice { npc_id, think_id, invader: false, label: 0, friendly: false })
        .collect();
    start_mimic_watch(picks.clone())?;
    let mut slot = ENEMY_QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
    match slot.as_mut() {
        Some(queue) => {
            queue.remaining.extend(picks.iter().copied());
            queue.replacements = 0;
        }
        None => {
            *slot = Some(EnemyQueue {
                creator,
                routine,
                remaining: picks.clone(),
                pool: picks,
                replace_oldest: true,
                limit: 5,
                full_frames: 0,
                pause_frames: 0,
                replacements: 0,
                checks: Vec::new(),
                in_flight: None,
                started: 0,
            });
        }
    }
    Ok(format!(
        "spawning three Mimics ({npc_id}, AI {think_id}), logging for {MIMIC_TEST_SECONDS} seconds"
    ))
}

/// Starts logging. With `picks`, the spawned Mimics are moved into a row in front of you.
fn start_mimic_watch(picks: Vec<EnemyChoice>) -> Result<(), String> {
    let spots: Vec<([f32; 3], f32)> = if picks.is_empty() {
        Vec::new()
    } else {
        chest_row_spots()?
    };
    *MIMIC_WATCH.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(MimicWatch {
        started: Instant::now(),
        last_log: Instant::now(),
        before: spawned_chrs(),
        last_player_anim: 0,
        last_anims: Vec::new(),
        spots,
        placed: Vec::new(),
        place_frames: 0,
        hit: None,
    });
    Ok(())
}

/// Characters this close are logged by the Mimic test, map enemies too.
const NEARBY_METRES: f32 = 6.0;

/// Every character the game has loaded: the map's, the spawned ones and the other sets.
fn all_chrs() -> Vec<usize> {
    let Ok(world) = (unsafe { WorldChrMan::instance() }) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let slots = world.world_block_chr.as_ptr() as usize;
    let slot_size = std::mem::size_of_val(&world.world_block_chr) / world.world_block_chr.len();
    for index in 0..world.world_block_chr.len() {
        let block = slots + index * slot_size;
        let capacity = read_u32_checked(block + BLOCK_CHR_SET_CAPACITY).unwrap_or(0) as usize;
        let entries = read_usize_checked(block + BLOCK_CHR_SET_ENTRIES).unwrap_or(0);
        if capacity == 0 || capacity > MAX_CHRS_PER_BLOCK as usize || entries < 0x10_000 {
            continue;
        }
        for entry in 0..capacity {
            let chr = read_usize_checked(entries + entry * CHR_SET_ENTRY_SIZE).unwrap_or(0);
            if chr >= 0x10_000 {
                found.push(chr);
            }
        }
    }
    let world = world as *const WorldChrMan as usize;
    found.extend(chr_set_members(world + DEBUG_CHR_SET));
    for offset in OTHER_CHR_SETS {
        found.extend(chr_set_members(world + offset));
    }
    found.sort_unstable();
    found.dedup();
    found
}

fn distance_between(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Characters other than the player within `metres`, closest first.
fn chrs_near_player(metres: f32) -> Vec<(usize, f32)> {
    let Ok(player) = local_player() else {
        return Vec::new();
    };
    let player = player as usize;
    let Some(here) = chr_position(player) else {
        return Vec::new();
    };
    let mut near: Vec<(usize, f32)> = all_chrs()
        .into_iter()
        .filter(|chr| *chr != player)
        .filter_map(|chr| chr_position(chr).map(|there| (chr, distance_between(here, there))))
        .filter(|(_, distance)| *distance <= metres)
        .collect();
    near.sort_by(|a, b| a.1.total_cmp(&b.1));
    near
}

/// The Mimic's grabs in the game's throw list, from the game's own names. The last two are
/// the bite from the closed chest ("Mimicry").
const MIMIC_THROWS: [(u32, &str); 5] = [
    (521_200, "Hug"),
    (521_201, "Jump"),
    (521_202, "Back Bite"),
    (521_203, "Mimicry"),
    (521_204, "Mimicry"),
];

/// Writes the Mimic's grabs to the log: the Mimic's animation and the player's for each.
fn log_mimic_throws() {
    let Ok(regulation) = (unsafe { CSRegulationManager::instance() }) else {
        return;
    };
    let throws = regulation.get_param::<THROW_INFO_BANK>();
    for (id, name) in MIMIC_THROWS {
        match throws.get(u64::from(id)) {
            Some(row) => crate::log::log(format!(
                "mimic test: throw {id} {name}: Mimic animation {}, player animation {}, distance {:.2}, type {}",
                row.atk_anim_id(),
                row.def_anim_id(),
                row.dist(),
                row.throw_type_id(),
            )),
            None => crate::log::log(format!("mimic test: throw {id} {name} is missing")),
        }
    }
}

/// The routine behind the game's ForcePlayAnimation event command: (character, animation,
/// two flags). It marks the character and passes the animation to its event module. The
/// event command reaches it through an object only map-placed characters have, so the mod
/// calls it directly, which works for spawned characters too.
const FORCE_PLAY_ANIMATION_PATTERN: &str =
    "48 85 C9 74 ?? 80 A1 09 1A 00 00 FD 41 80 E1 01 45 02 C9 44 08 89 09 1A 00 00 48 8B 89 90 1F 00 00 48 8B 49 58 E9";

fn force_play_animation_routine() -> Result<usize, String> {
    static ROUTINE: OnceLock<Result<usize, String>> = OnceLock::new();
    ROUTINE.get_or_init(|| find_unique(FORCE_PLAY_ANIMATION_PATTERN)).clone()
}

/// Plays an animation on any character the way the game's event scripts force one,
/// interrupting whatever it was doing. Must run on the game's thread.
fn request_animation(chr: usize, animation: i32) -> Result<(), String> {
    request_animation_with(chr, animation, 0, 0)
}

fn request_animation_with(chr: usize, animation: i32, first: u8, second: u8) -> Result<(), String> {
    let routine = force_play_animation_routine()?;
    let play: extern "C" fn(usize, i32, u8, u8) = unsafe { std::mem::transmute(routine) };
    play(chr, animation, first, second);
    Ok(())
}

/// Ids the scan tries: the Mimic's own animations seen in the logs, the grabs from the
/// throw list, and the same with the 2000000 the chest pose's ids carry.
const SCAN_ANIMATIONS: [i32; 22] = [
    7000, 2000020, 2003010, 3000, 3004, 4100, 4110, 5003, 9500, 2000000, 2000010, 2001000,
    2002000, 2003000, 2003020, 2004000, 2004100, 2004110, 2005000, 2006000, 2007000, 2009000,
];
const SCAN_FLAGS: [(u8, u8); 2] = [(0, 0), (0, 1)];
/// A sleeping Mimic shows its animations with this added: asking for 3000 plays 2003000.
const CHEST_GROUP: i32 = 2_000_000;
/// Frames between asking and reading back, and between one try and the next.
const SCAN_SETTLE_FRAMES: u32 = 20;
const SCAN_STEP_FRAMES: u32 = 45;

struct AnimationScan {
    chr: usize,
    /// Each try: the animation and the two flags.
    tries: Vec<(i32, u8, u8)>,
    step_frames: u32,
    step: usize,
    frame: u32,
    before: i32,
}

static ANIMATION_SCAN: Mutex<Option<AnimationScan>> = Mutex::new(None);

/// Developer test: tries every id in [SCAN_ANIMATIONS] on the closest character, one after
/// another, and logs which ones it really plays.
pub fn scan_nearest_animations() -> Result<String, String> {
    let (chr, distance) = *chrs_near_player(NEARBY_METRES)
        .first()
        .ok_or_else(|| format!("nothing is within {NEARBY_METRES} m"))?;
    let tries: Vec<(i32, u8, u8)> = SCAN_ANIMATIONS
        .iter()
        .flat_map(|animation| SCAN_FLAGS.iter().map(|(first, second)| (*animation, *first, *second)))
        .collect();
    start_animation_scan(chr, tries, SCAN_STEP_FRAMES)?;
    let count = SCAN_ANIMATIONS.len() * SCAN_FLAGS.len();
    Ok(format!(
        "trying {count} animations on {chr:#x} ({distance:.1} m away), about {} seconds",
        count as u32 * SCAN_STEP_FRAMES / 60
    ))
}

fn start_animation_scan(chr: usize, tries: Vec<(i32, u8, u8)>, step_frames: u32) -> Result<(), String> {
    force_play_animation_routine()?;
    *ANIMATION_SCAN.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(AnimationScan {
        chr,
        tries,
        step_frames,
        step: 0,
        frame: 0,
        before: chr_anim(chr).unwrap_or(-1),
    });
    Ok(())
}

/// Developer test: plays the listed animations on the closest character, one after
/// another with `gap_frames` between them, and logs what it really plays.
pub fn play_nearest_sequence(text: &str, gap_frames: u32) -> Result<String, String> {
    let animations = parse_npc_param_ids(text);
    if animations.is_empty() {
        return Err("no animation ids given".to_string());
    }
    let (chr, distance) = *chrs_near_player(NEARBY_METRES)
        .first()
        .ok_or_else(|| format!("nothing is within {NEARBY_METRES} m"))?;
    let tries = animations.iter().map(|animation| (*animation as i32, 0, 0)).collect();
    start_animation_scan(chr, tries, gap_frames.max(SCAN_SETTLE_FRAMES + 1))?;
    Ok(format!("playing {animations:?} on {chr:#x} ({distance:.1} m away), {gap_frames} frames apart"))
}

pub fn poll_animation_scan() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = ANIMATION_SCAN.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(scan) = slot.as_mut() else {
            return;
        };
        if scan.step >= scan.tries.len() || !all_chrs().contains(&scan.chr) {
            crate::log::log("animation scan: done".to_string());
            *slot = None;
            return;
        }
        let (animation, first, second) = scan.tries[scan.step];
        if scan.frame == 0 {
            scan.before = chr_anim(scan.chr).unwrap_or(-1);
            if let Err(error) = request_animation_with(scan.chr, animation, first, second) {
                crate::log::log(format!("animation scan: {animation} failed: {error}"));
            }
        }
        if scan.frame == SCAN_SETTLE_FRAMES {
            let now = chr_anim(scan.chr).unwrap_or(-1);
            let verdict = if now == animation || now == animation + CHEST_GROUP { "PLAYS" } else { "no" };
            crate::log::log(format!(
                "animation scan: {animation} flags {first}/{second}: {verdict} (was {}, now {now})",
                scan.before
            ));
        }
        scan.frame += 1;
        if scan.frame >= scan.step_frames {
            scan.frame = 0;
            scan.step += 1;
        }
    }));
}

/// Developer test: makes the closest character play an animation, the way the game's
/// event scripts ask for one, and the player another one at the same moment if asked.
pub fn force_nearest_animation(animation: i32, player_animation: i32) -> Result<String, String> {
    let (chr, distance) = *chrs_near_player(NEARBY_METRES)
        .first()
        .ok_or_else(|| format!("nothing is within {NEARBY_METRES} m"))?;
    request_animation(chr, animation)?;
    if player_animation > 0 {
        request_animation(local_player()? as usize, player_animation)?;
        return Ok(format!(
            "asked {chr:#x} ({distance:.1} m away) to play {animation} and the player {player_animation}"
        ));
    }
    Ok(format!("asked {chr:#x} ({distance:.1} m away) to play {animation}"))
}

fn chr_anim(chr: usize) -> Option<i32> {
    let action = module_pointer(chr, ACTION_REQUEST_OFFSET).ok()?;
    read_u32_checked(action + CURRENT_ANIM_OFFSET).ok().map(|anim| anim as i32)
}

fn chr_position(chr: usize) -> Option<[f32; 3]> {
    let physics = module_pointer(chr, PHYSICS_MODULE_OFFSET).ok()?;
    let mut bytes = [0u8; 12];
    (copy_readable(physics + PHYSICS_POSITION, &mut bytes) == 12).then(|| {
        [0, 1, 2].map(|axis| f32::from_ne_bytes(bytes[axis * 4..axis * 4 + 4].try_into().unwrap()))
    })
}

/// The ways a facing angle can stand for a direction on the ground (x, z): sine or cosine
/// for x, and the signs. Which one the game uses is read off the player, whose angle and
/// direction are both known, instead of being assumed.
const FACING_CONVENTIONS: usize = 8;

fn facing_direction(convention: usize, angle: f32) -> [f32; 2] {
    let (sx, sz) = ([1.0, -1.0][convention & 1], [1.0, -1.0][(convention >> 1) & 1]);
    if convention & 4 == 0 {
        [sx * angle.sin(), sz * angle.cos()]
    } else {
        [sx * angle.cos(), sz * angle.sin()]
    }
}

/// The angle that looks along (x, z) under `convention`.
pub fn facing_for(convention: usize, x: f32, z: f32) -> f32 {
    let (sx, sz) = ([1.0, -1.0][convention & 1], [1.0, -1.0][(convention >> 1) & 1]);
    if convention & 4 == 0 {
        (sx * x).atan2(sz * z)
    } else {
        (sz * z).atan2(sx * x)
    }
}

/// The convention under which `angle` points closest to `forward`.
pub fn pick_facing_convention(angle: f32, forward: [f32; 2]) -> usize {
    (0..FACING_CONVENTIONS)
        .min_by(|a, b| {
            let miss = |convention: usize| {
                let [x, z] = facing_direction(convention, angle);
                (x - forward[0]).powi(2) + (z - forward[1]).powi(2)
            };
            miss(*a).total_cmp(&miss(*b))
        })
        .unwrap_or(0)
}

fn player_facing_convention() -> Result<usize, String> {
    let player = local_player()? as usize;
    let physics = module_pointer(player, PHYSICS_MODULE_OFFSET)?;
    let angle = read_f32_checked(physics + PHYSICS_ROTATION + 4)?;
    let ctrl = read_usize_checked(player + CHR_CTRL_OFFSET)?;
    let forward = [-read_f32_checked(ctrl + CTRL_FORWARD[0])?, -read_f32_checked(ctrl + CTRL_FORWARD[2])?];
    Ok(pick_facing_convention(angle, forward))
}

/// Moves each new Mimic to its spot in the row, facing the player.
fn place_mimics(watch: &mut MimicWatch, mimics: &[usize]) {
    for chr in mimics {
        if !watch.placed.contains(chr) && watch.placed.len() < watch.spots.len() {
            watch.placed.push(*chr);
            watch.place_frames = 0;
        }
    }
    if watch.place_frames >= MIMIC_PLACE_FRAMES {
        return;
    }
    watch.place_frames += 1;
    for (chr, (spot, facing)) in watch.placed.iter().zip(&watch.spots) {
        let Ok(physics) = module_pointer(*chr, PHYSICS_MODULE_OFFSET) else {
            continue;
        };
        for (axis, value) in spot.iter().enumerate() {
            let _ = write_bytes_checked(physics + PHYSICS_POSITION + axis * 4, &value.to_ne_bytes());
        }
        let _ = write_bytes_checked(physics + PHYSICS_ROTATION + 4, &facing.to_ne_bytes());
        if watch.place_frames == MIMIC_PLACE_FRAMES {
            crate::log::log(format!(
                "mimic test: placed {chr:#x} at {spot:?} facing {facing:.2}, effects {:?}",
                chr_sp_effects(*chr)
            ));
        }
    }
}

/// Logs any change of animation right away, and positions twice a second.
pub fn poll_mimic_test() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut slot = MIMIC_WATCH.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(watch) = slot.as_mut() else {
            return;
        };
        if watch.started.elapsed() > Duration::from_secs(MIMIC_TEST_SECONDS) {
            crate::log::log("mimic test: done logging".to_string());
            *slot = None;
            return;
        }
        let Ok(player) = local_player() else {
            return;
        };
        let player = player as usize;
        let player_anim = chr_anim(player).unwrap_or(-1);
        if player_anim != watch.last_player_anim {
            crate::log::log(format!("mimic test: player animation {player_anim}"));
            watch.last_player_anim = player_anim;
        }
        let here = chr_position(player);
        let mimics: Vec<usize> = spawned_chrs().into_iter().filter(|chr| !watch.before.contains(chr)).collect();
        place_mimics(watch, &mimics);
        if watch.hit.is_none() && !watch.spots.is_empty() {
            // They all start with the same health, so one below the rest was hit.
            let full = mimics.iter().filter_map(|chr| chr_health(*chr)).max().unwrap_or(0);
            if let Some(hit) = mimics.iter().copied().find(|chr| chr_health(*chr).is_some_and(|hp| hp < full)) {
                watch.hit = Some(hit);
                for other in mimics.iter().copied().filter(|chr| *chr != hit) {
                    let woken = request_animation(other, MIMIC_WAKE_ANIMATION);
                    crate::log::log(format!("mimic test: {hit:#x} was hit, waking {other:#x}: {woken:?}"));
                }
            }
        }
        let mut lines = Vec::new();
        let mut watched: Vec<usize> = chrs_near_player(NEARBY_METRES).into_iter().map(|(chr, _)| chr).collect();
        for chr in &mimics {
            if !watched.contains(chr) {
                watched.push(*chr);
            }
        }
        for chr in &watched {
            let anim = chr_anim(*chr).unwrap_or(-1);
            let previous = watch.last_anims.iter().find(|(known, _)| known == chr).map(|(_, anim)| *anim);
            if previous != Some(anim) {
                crate::log::log(format!("mimic test: {chr:#x} animation {anim}, effects {:?}", chr_sp_effects(*chr)));
                watch.last_anims.retain(|(known, _)| known != chr);
                watch.last_anims.push((*chr, anim));
            }
            let metres = here.zip(chr_position(*chr)).map(|(a, b)| {
                ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
            });
            let hp = chr_health(*chr).unwrap_or(-1);
            lines.push(format!("{chr:#x} {:.1} m hp {hp}", metres.unwrap_or(-1.0)));
        }
        if watch.last_log.elapsed() >= Duration::from_millis(500) && !lines.is_empty() {
            watch.last_log = Instant::now();
            crate::log::log(format!("mimic test: {}", lines.join(", ")));
        }
    }));
}

fn invader_candidates(ashes: bool, ringed_city: bool) -> Vec<(u32, &'static str)> {
    let mut list = BASE_INVADERS.to_vec();
    if ashes {
        list.extend(ASHES_OF_ARIANDEL_INVADERS);
    }
    if ringed_city {
        list.extend(RINGED_CITY_INVADERS);
    }
    list
}

fn think_for_invader(
    npc_id: u32,
    ai_think_id: i32,
    init_think_id: i32,
    think_ids: &[u32],
) -> Option<u32> {
    // Human NPCs keep their fight AI on CharaInit, not on NpcParam.
    for candidate in [init_think_id, ai_think_id] {
        if let Ok(id) = u32::try_from(candidate) {
            if id > 0 && think_ids.binary_search(&id).is_ok() {
                return Some(id);
            }
        }
    }
    think_candidates(npc_id, think_ids).into_iter().next()
}

/// The fight AI row a human NPC's CharaInit row names, or 0. A missing row means the NPC
/// cannot be built as a player-type character at all.
fn invader_init_think(npc_id: u32) -> Option<i32> {
    let regulation = unsafe { CSRegulationManager::instance() }.ok()?;
    let inits = regulation.get_param::<CHARACTER_INIT_PARAM>();
    inits.get(u64::from(npc_id)).map(|row| row.npc_player_think_id())
}

/// An invader that is a dark spirit in the game's own data, used as the model for the look.
const DARK_SPIRIT_LOOK_FROM: u32 = 29_440;

/// The two CharaInit fields that decide what kind of phantom an NPC is built as and how it
/// is drawn: white for a summon, red for an invader, and so on.
fn npc_look(npc_id: u32) -> Option<(u8, u8)> {
    let regulation = unsafe { CSRegulationManager::instance() }.ok()?;
    let row = regulation.get_param::<CHARACTER_INIT_PARAM>().get(u64::from(npc_id))?;
    Some((row.npc_player_type(), row.npc_player_draw_type()))
}

fn set_npc_look(npc_id: u32, look: (u8, u8)) {
    let Ok(regulation) = (unsafe { CSRegulationManager::instance_mut() }) else {
        return;
    };
    if let Some(row) = regulation.get_mut_param::<CHARACTER_INIT_PARAM>().get_mut(u64::from(npc_id)) {
        row.set_npc_player_type(look.0);
        row.set_npc_player_draw_type(look.1);
    }
}

/// Some of the named NPCs are set up as summons or neutral characters, so they would arrive
/// with a white glow. For the moment it is built, the NPC's row gets the look of a real
/// dark spirit. Returns the row's own look to put back, or none if nothing was changed.
fn borrow_dark_spirit_look(npc_id: u32) -> Option<(u8, u8)> {
    let own = npc_look(npc_id)?;
    let dark_spirit = npc_look(DARK_SPIRIT_LOOK_FROM)?;
    if own == dark_spirit {
        return None;
    }
    crate::log::log(format!(
        "invader {npc_id}: look {own:?} -> {dark_spirit:?} (taken from {DARK_SPIRIT_LOOK_FROM})"
    ));
    set_npc_look(npc_id, dark_spirit);
    Some(own)
}

/// The NPC row fields that decide which side a human NPC is on and how it is drawn as a
/// phantom: team, phantom look, draw type and NPC type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NpcSide {
    team: u8,
    phantom: i32,
    draw: u8,
    kind: u8,
}

fn npc_side(npc_id: u32) -> Option<NpcSide> {
    let regulation = unsafe { CSRegulationManager::instance() }.ok()?;
    let row = regulation.get_param::<NPC_PARAM_ST>().get(u64::from(npc_id))?;
    Some(NpcSide {
        team: row.team_type(),
        phantom: row.phantom_param_id(),
        draw: row.draw_type(),
        kind: row.npc_type(),
    })
}

fn set_npc_side(npc_id: u32, side: NpcSide) {
    let Ok(regulation) = (unsafe { CSRegulationManager::instance_mut() }) else {
        return;
    };
    if let Some(row) = regulation.get_mut_param::<NPC_PARAM_ST>().get_mut(u64::from(npc_id)) {
        row.set_team_type(side.team);
        row.set_phantom_param_id(side.phantom);
        row.set_draw_type(side.draw);
        row.set_npc_type(side.kind);
    }
}

/// The side of a white phantom summon, as the game's own summonable NPCs have it.
const WHITE_PHANTOM_SIDE: NpcSide = NpcSide { team: CHR_TEAM_WHITE_PHANTOM as u8, phantom: 80, draw: 0, kind: 0 };
/// What the game's own invaders have, in case the model row is missing.
const DARK_SPIRIT_SIDE: NpcSide = NpcSide { team: CHR_TEAM_DARK_SPIRIT as u8, phantom: 81, draw: 0, kind: 0 };
/// PlayerIns: its NPC row lookup, an id followed by a pointer to the row.
const NPC_ROW_LOOKUP: usize = 0x2038;
const NPC_ROW_LOOKUP_ROW: usize = 0x08;

/// Every human NPC (one with a CharaInit row of the same id) and the side the game's data
/// gives it. Read once, before the mod changes any row, so it stays the game's own view.
fn natural_sides() -> &'static [(u32, NpcSide)] {
    static SIDES: OnceLock<Vec<(u32, NpcSide)>> = OnceLock::new();
    SIDES.get_or_init(|| {
        let Ok(regulation) = (unsafe { CSRegulationManager::instance() }) else {
            return Vec::new();
        };
        let npcs = regulation.get_param::<NPC_PARAM_ST>();
        regulation
            .get_param::<CHARACTER_INIT_PARAM>()
            .iter()
            .filter_map(|(id, _)| {
                let row = npcs.get(id)?;
                let side = NpcSide {
                    team: row.team_type(),
                    phantom: row.phantom_param_id(),
                    draw: row.draw_type(),
                    kind: row.npc_type(),
                };
                Some((id as u32, side))
            })
            .collect()
    })
}

/// The NPCs the game itself sets up as white phantom summons. They come with the right
/// side and with an AI that follows the host and fights enemies.
pub fn is_summon_side(team: u8, phantom: i32) -> bool {
    u32::from(team) == CHR_TEAM_WHITE_PHANTOM && phantom == WHITE_PHANTOM_SIDE.phantom
}

fn wanted_side(friendly: bool) -> NpcSide {
    if friendly {
        WHITE_PHANTOM_SIDE
    } else {
        npc_side(DARK_SPIRIT_LOOK_FROM).unwrap_or(DARK_SPIRIT_SIDE)
    }
}

/// The named NPCs are a mix of invaders, summons and neutral characters in the game's data,
/// and the glow follows the NPC row: red for a dark spirit, white for a summon. Writing
/// onto the finished character changes who it fights but not how it is drawn, and the
/// game reads the phantom look from the row again after the character is built. So the
/// row itself gets the wanted side and keeps it until the game is closed, or until the
/// same NPC is sent for the other side. Nothing is written to disk.
fn set_side_in_npc_row(npc_id: u32, side: NpcSide) {
    let Some(own) = npc_side(npc_id) else {
        return;
    };
    if own != side {
        crate::log::log(format!("npc {npc_id}: side {own:?} -> {side:?}"));
        set_npc_side(npc_id, side);
    }
}

/// Gives the character its own copy of its NPC row, taken while the row holds the wanted
/// side. Then a friend and a foe built from the same NPC can stand next to each other, as
/// far as the game reads the row through the character.
fn give_own_npc_row(chr: usize) -> Result<(), String> {
    let lookup = read_usize_checked(chr + NPC_ROW_LOOKUP)?;
    if lookup < 0x10_000 {
        return Err("the character has no NPC row lookup".to_string());
    }
    let row = read_usize_checked(lookup + NPC_ROW_LOOKUP_ROW)?;
    let copy = leaked(std::mem::size_of::<NPC_PARAM_ST>());
    if row < 0x10_000 || copy_readable(row, copy) != copy.len() {
        return Err("the NPC row is not readable".to_string());
    }
    write_bytes_checked(lookup + NPC_ROW_LOOKUP_ROW, &(copy.as_ptr() as usize).to_ne_bytes())
}

// Map data as the game keeps it in memory, and one enemy entry in it. The factory and the
// player constructor read these fields; everything else comes from a real entry of the map.
const MAP_DATA_COPY_SIZE: usize = 0x600;
/// The map id at 0x4E8 is the last field the constructors read.
const MAP_DATA_MIN_SIZE: usize = 0x4F0;
/// All entries of the map: count plus one at +4, pointers from +0x10.
const MAP_DATA_ENTRY_TABLE: usize = 0x80;
const MAP_DATA_MODEL_TABLE: usize = 0x68;
/// Enemy entries: count, index of the first one among all entries, pointer array.
const MAP_DATA_ENEMY_COUNT: usize = 0x3C8;
const MAP_DATA_ENEMY_FIRST: usize = 0x3CC;
const MAP_DATA_ENEMY_ARRAY: usize = 0x3D0;
const MAP_ENTRY_COPY_SIZE: usize = 0x100;
const MAP_ENTRY_MIN_SIZE: usize = 0xC0;
const MAP_ENTRY_NAME: usize = 0x00;
const MAP_ENTRY_TYPE: usize = 0x08;
const MAP_ENTRY_TYPE_ENEMY: u32 = 2;
const MAP_ENTRY_MODEL_INDEX: usize = 0x10;
const MAP_ENTRY_POSITION: usize = 0x20;
const MAP_ENTRY_ROTATION: usize = 0x2C;
/// Eight group words. All bits set shows the character from everywhere, like a debug spawn.
const MAP_ENTRY_DRAW_GROUPS: usize = 0x4C;
const MAP_ENTRY_ENTITY: usize = 0xB0;
const MAP_ENTRY_ENEMY_DATA: usize = 0xB8;
const ENTITY_COPY_SIZE: usize = 0x60;
/// Event scripts find characters by these ids, so the invader must not inherit any. The
/// entry's own id is first. Its group ids follow a block of one-byte settings, which must
/// stay as they are: some of them decide whether the character is drawn at all.
const ENTITY_GROUPS_FROM: usize = 0x18;
/// Map ids have seven digits. Packed one-byte settings read as a number are either far
/// smaller or use the top byte.
const ENTITY_ID_RANGE: std::ops::Range<u32> = 1_000_000..0x0100_0000;
const ENEMY_DATA_COPY_SIZE: usize = 0x80;
const ENEMY_DATA_THINK: usize = 0x08;
const ENEMY_DATA_NPC: usize = 0x0C;
const ENEMY_DATA_TALK: usize = 0x10;
const ENEMY_DATA_CHARA_INIT: usize = 0x18;
const ENEMY_DATA_COLLISION: usize = 0x1C;
/// Inside the creator routine: the jump to the create command.
const CREATE_COMMAND_JUMP: usize = 0x8A;
/// Inside the create command: where it loads the list of loaded map blocks.
const CREATE_BLOCK_LIST_LOAD: usize = 0x114;
const BLOCK_LIST: usize = 0x10;
const BLOCK_LIST_CURRENT: usize = 0x20;
const BLOCK_LIST_COUNT: usize = 0x18;
const BLOCK_LIST_ENTRIES: usize = 0x20;
const BLOCK_ENTRY_SIZE: usize = 0x70;
const BLOCK_ENTRY_MAP_DATA: usize = 0x28;
/// The creator keeps the map data of its last spawn here.
const CREATOR_MAP_DATA: usize = 0x1A0;
/// WorldChrMan: the set that holds spawned characters. Capacity, then the entries.
const DEBUG_CHR_SET: usize = 0x68;
/// WorldChrMan: world info, which counts the loaded blocks. Spawned characters use the
/// block number two past that count.
const WORLD_INFO: usize = 0x08;
const WORLD_INFO_BLOCK_COUNT: usize = 0x18;
const DEBUG_BLOCK_OFFSET: u32 = 2;
/// Data module: the map data and entry index a character was built from.
const CHR_DATA_MAP_DATA: usize = 0x10;
const CHR_DATA_MAP_ENTRY: usize = 0x18;
const PHYSICS_MODULE_OFFSET: usize = 0x68;
/// Module list: the character's AI.
const AI_MODULE_OFFSET: usize = 0x38;
/// Dump phantoms also writes down the enemies this close to the player, at most this many.
const DUMP_ENEMY_RANGE: f32 = 40.0;
const DUMP_ENEMIES: usize = 20;
const PHYSICS_ROTATION: usize = 0x70;
const PHYSICS_POSITION: usize = 0x80;
/// Character controller: the columns of its facing matrix. Forward is their negative.
const CTRL_FORWARD: [usize; 3] = [0x128, 0x138, 0x148];

fn put_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn put_f32(buffer: &mut [u8], offset: usize, value: f32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn put_usize(buffer: &mut [u8], offset: usize, value: usize) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_ne_bytes());
}

fn get_u32(buffer: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(buffer[offset..offset + 4].try_into().unwrap())
}

fn get_usize(buffer: &[u8], offset: usize) -> usize {
    usize::from_ne_bytes(buffer[offset..offset + 8].try_into().unwrap())
}

/// A zeroed block that is never freed. The game may keep pointers into the made-up map
/// entry, and a few kilobytes per invader is nothing next to a dangling pointer.
fn leaked(size: usize) -> &'static mut [u8] {
    Box::leak(vec![0u8; size].into_boxed_slice())
}

fn read_f32_checked(address: usize) -> Result<f32, String> {
    read_u32_checked(address).map(f32::from_bits)
}

/// An entity struct copied from a real enemy, with its id and group ids taken out.
pub fn clear_entity_ids(entity: &mut [u8]) {
    put_u32(entity, 0, u32::MAX);
    for offset in (ENTITY_GROUPS_FROM..entity.len().saturating_sub(3)).step_by(4) {
        if ENTITY_ID_RANGE.contains(&get_u32(entity, offset)) {
            put_u32(entity, offset, u32::MAX);
        }
    }
}

/// The map data of the block the player is in, found the way the creator's own create
/// command finds it.
fn current_map_data(creator: usize) -> Result<usize, String> {
    let from_blocks = || -> Result<usize, String> {
        let create = jump_target(create_npc_address()? + CREATE_COMMAND_JUMP)?;
        let site = create + CREATE_BLOCK_LIST_LOAD;
        let mut bytes = [0u8; 3];
        if copy_readable(site, &mut bytes) != bytes.len() || bytes != [0x48, 0x8B, 0x05] {
            return Err("the create command does not load the block list where expected".to_string());
        }
        let owner = read_usize_checked(pattern::rip_target(site, 3, 7)?)?;
        let list = read_usize_checked(owner + BLOCK_LIST)?;
        let current = read_u32_checked(owner + BLOCK_LIST_CURRENT)? as i32;
        let count = read_u32_checked(list + BLOCK_LIST_COUNT)? as i32;
        if current < 0 || current >= count {
            return Err("no current map block".to_string());
        }
        let entries = read_usize_checked(list + BLOCK_LIST_ENTRIES)?;
        read_usize_checked(entries + current as usize * BLOCK_ENTRY_SIZE + BLOCK_ENTRY_MAP_DATA)
    };
    let map = match from_blocks() {
        Ok(map) if map >= 0x10_000 => map,
        _ => read_usize_checked(creator + CREATOR_MAP_DATA)?,
    };
    if map < 0x10_000 {
        return Err("the current map data was not found".to_string());
    }
    Ok(map)
}

/// A real enemy entry of the map to copy the unknown fields from.
fn template_map_entry(map: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), String> {
    let count = (get_u32(map, MAP_DATA_ENEMY_COUNT) as usize).min(MAX_CHRS_PER_BLOCK as usize);
    let array = get_usize(map, MAP_DATA_ENEMY_ARRAY);
    let models = get_usize(map, MAP_DATA_MODEL_TABLE);
    let model_count = read_u32_checked(models + 4).unwrap_or(0) as usize;
    for index in 0..count {
        let Ok(entry_address) = read_usize_checked(array + index * 8) else {
            continue;
        };
        let mut entry = vec![0u8; MAP_ENTRY_COPY_SIZE];
        if entry_address < 0x10_000 || copy_readable(entry_address, &mut entry) < MAP_ENTRY_MIN_SIZE {
            continue;
        }
        let model = get_u32(&entry, MAP_ENTRY_MODEL_INDEX) as usize;
        if get_u32(&entry, MAP_ENTRY_TYPE) != MAP_ENTRY_TYPE_ENEMY
            || model + 1 >= model_count
            || read_usize_checked(models + 0x10 + model * 8).unwrap_or(0) < 0x10_000
        {
            continue;
        }
        let mut entity = vec![0u8; ENTITY_COPY_SIZE];
        let mut enemy = vec![0u8; ENEMY_DATA_COPY_SIZE];
        let entity_address = get_usize(&entry, MAP_ENTRY_ENTITY);
        let enemy_address = get_usize(&entry, MAP_ENTRY_ENEMY_DATA);
        if entity_address < 0x10_000
            || enemy_address < 0x10_000
            || copy_readable(entity_address, &mut entity) != entity.len()
            || copy_readable(enemy_address, &mut enemy) != enemy.len()
        {
            continue;
        }
        return Ok((entry, entity, enemy));
    }
    Err("this map has no enemy entry to model the invader on".to_string())
}

/// Where the invader appears and which way it faces: a few meters ahead, looking back.
fn invader_spawn_point(distance: f32) -> Result<([f32; 3], f32), String> {
    let player = local_player()? as usize;
    let physics = module_pointer(player, PHYSICS_MODULE_OFFSET)?;
    let mut position = [0f32; 3];
    for (axis, value) in position.iter_mut().enumerate() {
        *value = read_f32_checked(physics + PHYSICS_POSITION + axis * 4)?;
    }
    let facing = read_f32_checked(physics + PHYSICS_ROTATION + 4)?;
    let ctrl = read_usize_checked(player + CHR_CTRL_OFFSET)?;
    let mut forward = [0f32; 3];
    for (axis, offset) in CTRL_FORWARD.iter().enumerate() {
        forward[axis] = -read_f32_checked(ctrl + offset)?;
    }
    let length = (forward[0] * forward[0] + forward[2] * forward[2]).sqrt();
    if !length.is_finite() || !(0.5..=1.5).contains(&length) {
        forward = [facing.sin(), 0.0, facing.cos()];
    }
    if position.iter().any(|value| !value.is_finite()) || !facing.is_finite() {
        return Err("the player position is not readable".to_string());
    }
    Ok((
        [
            position[0] + forward[0] * distance,
            position[1] + SPAWN_HEIGHT_METERS,
            position[2] + forward[2] * distance,
        ],
        facing + std::f32::consts::PI,
    ))
}

/// Builds a human NPC as a player-type character, the way a map does when it loads one.
/// The game's factory reads a map entry, so it gets a made-up one: a copy of a real enemy
/// entry of this map with the invader's rows, position and no event ids. The new character
/// goes into the same set as debug spawns, so the limit, the checks and the game's delete
/// routine (which also gives back the NPC's player data slot) all work on it.
fn spawn_human_npc(queue: &EnemyQueue, enemy: EnemyChoice) -> Result<usize, String> {
    let located = cached()?;
    let factory = *located.map_chr_factory.as_ref().map_err(|error| error.clone())?;
    let place = *located.map_chr_place.as_ref().map_err(|error| error.clone())?;
    if invader_init_think(enemy.npc_id).is_none() {
        return Err(format!("NPC {} has no CharaInit row", enemy.npc_id));
    }
    let world = unsafe { WorldChrMan::instance() }
        .map_err(|error| format!("WorldChrMan is unavailable: {error:?}"))?
        as *const WorldChrMan as usize;
    let real_map = current_map_data(queue.creator)?;

    let set = world + DEBUG_CHR_SET;
    let capacity = read_u32_checked(set)? as usize;
    let entries = read_usize_checked(set + 8)?;
    if capacity == 0 || capacity > MAX_CHRS_PER_BLOCK as usize || entries < 0x10_000 {
        return Err("the spawned character set is not usable".to_string());
    }
    let index = (0..capacity)
        .find(|index| read_usize_checked(entries + index * CHR_SET_ENTRY_SIZE) == Ok(0))
        .ok_or("no free slot for a spawned character")?;
    let block = read_u32_checked(read_usize_checked(world + WORLD_INFO)? + WORLD_INFO_BLOCK_COUNT)?
        + DEBUG_BLOCK_OFFSET;

    let map = leaked(MAP_DATA_COPY_SIZE);
    if copy_readable(real_map, map) < MAP_DATA_MIN_SIZE {
        return Err("the current map data is not readable".to_string());
    }
    let (template, template_entity, template_enemy) = template_map_entry(map)?;
    let distance = SPAWN_DISTANCE_METERS + (queue.started % SPAWN_DISTANCE_STEPS) as f32;
    let (position, facing) = invader_spawn_point(distance)?;

    let name: Vec<u16> = format!("c0000_{:04}", 9000 + index).encode_utf16().chain([0]).collect();
    let name = Box::leak(name.into_boxed_slice());
    let entity = leaked(ENTITY_COPY_SIZE);
    entity.copy_from_slice(&template_entity);
    clear_entity_ids(entity);
    let data = leaked(ENEMY_DATA_COPY_SIZE);
    data.copy_from_slice(&template_enemy);
    put_u32(data, ENEMY_DATA_THINK, enemy.think_id);
    put_u32(data, ENEMY_DATA_NPC, enemy.npc_id);
    put_u32(data, ENEMY_DATA_TALK, 0);
    put_u32(data, ENEMY_DATA_CHARA_INIT, enemy.npc_id);
    put_u32(data, ENEMY_DATA_COLLISION, u32::MAX);
    let entry = leaked(MAP_ENTRY_COPY_SIZE);
    entry.copy_from_slice(&template);
    put_usize(entry, MAP_ENTRY_NAME, name.as_ptr() as usize);
    for (axis, value) in position.iter().enumerate() {
        put_f32(entry, MAP_ENTRY_POSITION + axis * 4, *value);
    }
    for (axis, value) in [0.0, facing, 0.0].iter().enumerate() {
        put_f32(entry, MAP_ENTRY_ROTATION + axis * 4, *value);
    }
    for word in 0..8 {
        put_u32(entry, MAP_ENTRY_DRAW_GROUPS + word * 4, u32::MAX);
    }
    put_usize(entry, MAP_ENTRY_ENTITY, entity.as_ptr() as usize);
    put_usize(entry, MAP_ENTRY_ENEMY_DATA, data.as_ptr() as usize);

    // Every index leads to the one entry, so the set slot can double as the entry index.
    let table = leaked(0x10 + capacity * 8);
    put_u32(table, 4, capacity as u32 + 2);
    for slot in 0..capacity {
        put_usize(table, 0x10 + slot * 8, entry.as_ptr() as usize);
    }
    put_usize(map, MAP_DATA_ENTRY_TABLE, table.as_ptr() as usize);
    put_u32(map, MAP_DATA_ENEMY_COUNT, capacity as u32);
    put_u32(map, MAP_DATA_ENEMY_FIRST, 0);
    put_usize(map, MAP_DATA_ENEMY_ARRAY, table.as_ptr() as usize + 0x10);

    let unused = 0usize;
    let build: extern "C" fn(usize, usize, usize, u32, usize, i32) -> usize =
        unsafe { std::mem::transmute(factory) };
    let own_look = borrow_dark_spirit_look(enemy.npc_id);
    // A real summon already has its side in the game's data. Only invaders need one given.
    if !enemy.friendly {
        set_side_in_npc_row(enemy.npc_id, wanted_side(false));
    } else if let Some((_, side)) = natural_sides().iter().find(|(id, _)| *id == enemy.npc_id) {
        // In case the same NPC was sent as an invader earlier in this session.
        set_side_in_npc_row(enemy.npc_id, *side);
    }
    let chr = build(
        &unused as *const usize as usize,
        0,
        entries + index * CHR_SET_ENTRY_SIZE,
        block,
        map.as_ptr() as usize,
        index as i32,
    );
    if let Some(look) = own_look {
        set_npc_look(enemy.npc_id, look);
    }
    if chr < 0x10_000 {
        return Err("the game did not build the NPC (all 40 human NPC slots may be in use)".to_string());
    }
    let finish: extern "C" fn(usize, usize, u32, usize, i32) = unsafe { std::mem::transmute(place) };
    finish(&unused as *const usize as usize, chr, block, map.as_ptr() as usize, index as i32);

    // From here on it should look like any spawned character: on the real map, with no
    // map entry behind it. Other code reads the map id through this pointer.
    let chr_data = module_pointer(chr, DATA_MODULE_OFFSET)?;
    write_bytes_checked(chr_data + CHR_DATA_MAP_DATA, &real_map.to_ne_bytes())?;
    write_i32_checked(chr_data + CHR_DATA_MAP_ENTRY, -1)?;
    if let Err(error) = give_own_npc_row(chr) {
        crate::log::log(format!("npc {}: shares its NPC row: {error}", enemy.npc_id));
    }
    Ok(chr)
}

/// What to announce for an invader that is still waiting to be built, by key. The queue
/// entries themselves are plain numbers.
struct InvaderNote {
    key: u32,
    /// The sender's name, or none to use the NPC's own.
    name: Option<String>,
    /// How long after the invasion message the invader appears.
    delay: Duration,
    /// The animation it appears with, or 0 for none.
    animation: i32,
    /// The NPC's own name in the game's text files, for the message when there is no
    /// sender's name and the mod does not know the name itself.
    npc_name_id: i32,
    /// When the message was shown. None until its turn comes.
    announced: Option<Instant>,
}

static INVADER_NOTES: Mutex<Vec<InvaderNote>> = Mutex::new(Vec::new());

fn keep_invader_note(name: Option<&str>, delay: Duration, animation: i32, npc_name_id: i32) -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let key = NEXT.fetch_add(1, Ordering::Relaxed).max(1);
    let name = name.map(str::trim).filter(|name| !name.is_empty()).map(str::to_string);
    let mut notes = INVADER_NOTES.lock().unwrap_or_else(|poison| poison.into_inner());
    notes.push(InvaderNote {
        key,
        name,
        delay,
        animation,
        npc_name_id,
        announced: None,
    });
    key
}

/// Announces an invader when its turn comes, then says whether its wait is over. Like a
/// real invasion: first the message, a moment later the invader.
fn invader_arrival_due(enemy: EnemyChoice) -> bool {
    let mut notes = INVADER_NOTES.lock().unwrap_or_else(|poison| poison.into_inner());
    let Some(note) = notes.iter_mut().find(|note| note.key == enemy.label) else {
        return true;
    };
    match note.announced {
        Some(announced) => announced.elapsed() >= note.delay,
        None => {
            let shown = match note.name.as_deref().or_else(|| known_invader_name(enemy.npc_id)) {
                Some(name) => ShownName::Text(name),
                None => ShownName::Npc(note.npc_name_id),
            };
            if let Err(error) = show_arrival_message(shown, enemy.friendly) {
                crate::log::log(format!("npc {}: arrival message not shown: {error}", enemy.npc_id));
            }
            note.announced = Some(Instant::now());
            note.delay.is_zero()
        }
    }
}

fn take_invader_note(key: u32) -> Option<InvaderNote> {
    let mut notes = INVADER_NOTES.lock().unwrap_or_else(|poison| poison.into_inner());
    let index = notes.iter().position(|note| note.key == key)?;
    Some(notes.remove(index))
}

/// The row the game uses when an NPC phantom arrives in the host's world, and the invade
/// type of an invading dark spirit, which picks the wording within that row. Together
/// they give "Invaded by dark spirit NAME!".
const ARRIVAL_MESSAGE_ROW: u32 = 0xCA;
const INVADE_TYPE_DARK_SPIRIT: u32 = 3;
/// The invade type of a summoned white phantom: "Phantom NAME has been summoned".
const INVADE_TYPE_WHITE_PHANTOM: u32 = 1;
/// Which of the routine's ways of putting the name in. The arrival lines use the first.
const ARRIVAL_NAME_SLOT: u32 = 1;
/// Name object: fixed text or null, then the wide string (text or pointer, length,
/// capacity, allocator). The fixed text wins when it is set, and a new object starts with
/// it pointing at an empty string.
const NAME_OBJECT_SIZE: usize = 0x40;
const NAME_FIXED_TEXT: usize = 0x00;
const NAME_TEXT: usize = 0x08;
const NAME_LENGTH: usize = 0x18;
const NAME_CAPACITY: usize = 0x20;
const NAME_ALLOCATOR: usize = 0x28;
/// The string keeps up to this many characters inside itself.
const NAME_INLINE_CAPACITY: usize = 7;
const ALLOCATOR_ALLOCATE: usize = 0x50;
/// Long enough for any Twitch name, short enough for the message bar.
pub const MESSAGE_NAME_LENGTH: usize = 32;

/// A name cut to what the game's field holds, as UTF-16 without splitting a character pair.
pub fn name_units(name: &str, limit: usize) -> Vec<u16> {
    let mut units = Vec::new();
    for character in name.chars().filter(|character| !character.is_control()) {
        let mut pair = [0u16; 2];
        let encoded = character.encode_utf16(&mut pair);
        if units.len() + encoded.len() > limit {
            break;
        }
        units.extend_from_slice(encoded);
    }
    units
}

/// Turns the memory at `base` into a name object holding `units`. Whoever receives it frees
/// longer text with the string's own allocator, so that allocator provides it.
fn fill_name_object(base: usize, units: &[u16]) -> Result<(), String> {
    let make = *cached()?.name_object.as_ref().map_err(|error| error.clone())?;
    let make_object: extern "C" fn(usize) -> usize = unsafe { std::mem::transmute(make) };
    make_object(base);
    let capacity = unsafe { ((base + NAME_CAPACITY) as *const usize).read() };
    let allocator = unsafe { ((base + NAME_ALLOCATOR) as *const usize).read() };
    if capacity != NAME_INLINE_CAPACITY || allocator < 0x10_000 {
        return Err("the name object does not look as expected".to_string());
    }
    let text = if units.len() <= NAME_INLINE_CAPACITY {
        base + NAME_TEXT
    } else {
        let allocate = read_usize_checked(read_usize_checked(allocator)? + ALLOCATOR_ALLOCATE)?;
        if !executable_address(allocate) {
            return Err("the string allocator is not usable".to_string());
        }
        let allocate: extern "C" fn(usize, usize, usize) -> usize = unsafe { std::mem::transmute(allocate) };
        let buffer = allocate(allocator, (units.len() + 1) * 2, 8);
        if buffer < 0x10_000 {
            return Err("no memory for the name".to_string());
        }
        unsafe {
            ((base + NAME_TEXT) as *mut usize).write(buffer);
            ((base + NAME_CAPACITY) as *mut usize).write(units.len());
        }
        buffer
    };
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), text as *mut u16, units.len());
        (text as *mut u16).add(units.len()).write(0);
        ((base + NAME_LENGTH) as *mut usize).write(units.len());
        ((base + NAME_FIXED_TEXT) as *mut usize).write(0);
    }
    Ok(())
}

/// The game's routine that turns a name in its text files into a name object:
/// `(name object, text id, with title) -> name object`. It is what gives an NPC the name
/// shown on lock-on, and it works before the NPC exists.
const NPC_NAME_PATTERN: &str = "40 55 56 57 48 8D 6C 24 C0 48 81 EC 40 01 00 00 48 C7 44 24 48 FE FF FF FF 48 89 9C 24 70 01 00 00 48 8B 05 ?? ?? ?? ?? 48 33 C4 48 89 45 30 41 0F B6 D8 48 8B F9 48 89 4C 24 40 33 F6 89 74 24 38";

fn npc_name_routine() -> Result<usize, String> {
    static ROUTINE: OnceLock<Result<usize, String>> = OnceLock::new();
    ROUTINE.get_or_init(|| find_unique(NPC_NAME_PATTERN)).clone()
}

/// The name in an arrival message.
enum ShownName<'a> {
    /// The sender's name, or a name the mod knows.
    Text(&'a str),
    /// The NPC's own name, by its text id in the game's files.
    Npc(i32),
}

/// Fills a new name object with an NPC's own name from the game's text files.
fn fill_npc_name_object(base: usize, text_id: i32) -> Result<(), String> {
    if text_id <= 0 {
        return Err("the NPC has no name in the game's text".to_string());
    }
    let make = *cached()?.name_object.as_ref().map_err(|error| error.clone())?;
    let routine = npc_name_routine()?;
    let make_object: extern "C" fn(usize) -> usize = unsafe { std::mem::transmute(make) };
    let fill: extern "C" fn(usize, i32, u8) -> usize = unsafe { std::mem::transmute(routine) };
    make_object(base);
    fill(base, text_id, 0);
    let length = unsafe { ((base + NAME_LENGTH) as *const usize).read() };
    if length > 0 {
        // The fixed text wins over the string when it is set, as in fill_name_object.
        unsafe { ((base + NAME_FIXED_TEXT) as *mut usize).write(0) };
        return Ok(());
    }
    // Or the routine pointed the fixed text at the name.
    let fixed = unsafe { ((base + NAME_FIXED_TEXT) as *const usize).read() };
    if fixed >= 0x10_000 && read_u16_checked(fixed).is_ok_and(|first| first != 0) {
        return Ok(());
    }
    Err(format!("text {text_id} gave an empty name"))
}

/// Shows the game's own arrival line, such as "Invaded by dark spirit NAME". The routine
/// frees the name object it is given.
fn show_arrival_message(name: ShownName, friendly: bool) -> Result<(), String> {
    let message = *cached()?.summon_message.as_ref().map_err(|error| error.clone())?;
    let owner = read_usize_checked(message.owner_slot)?;
    if owner < 0x10_000 {
        return Err("the message owner is not ready".to_string());
    }
    let mut object = [0u64; NAME_OBJECT_SIZE / 8];
    let base = object.as_mut_ptr() as usize;
    match name {
        ShownName::Text(name) => {
            let units = name_units(name, MESSAGE_NAME_LENGTH);
            if units.is_empty() {
                return Err("no name to show".to_string());
            }
            fill_name_object(base, &units)?;
        }
        ShownName::Npc(text_id) => fill_npc_name_object(base, text_id)?,
    }
    let show: extern "C" fn(usize, u32, u32, u32, usize, u32) = unsafe { std::mem::transmute(message.show) };
    let invade_type = if friendly { INVADE_TYPE_WHITE_PHANTOM } else { INVADE_TYPE_DARK_SPIRIT };
    show(owner, ARRIVAL_MESSAGE_ROW, invade_type, ARRIVAL_NAME_SLOT, base, 0);
    Ok(())
}

/// PlayerIns: its player data.
const PLAYER_GAME_DATA: usize = 0x1FA0;
/// The game's check for whether a character is simulated on this machine:
/// `(WorldChrMan, character id) -> bool`. When a hit lands, the game sorts attacker and
/// victim into "simulated here" or "simulated by another player" and only applies the damage
/// for some pairs. Characters a map places are in the registry this check reads. Spawned
/// ones are left out of it on purpose, so they count as another player's: their hits only
/// land on the local player, and two of them cannot hurt each other.
const LOCALLY_SIMULATED_PATTERN: &str =
    "48 83 EC 28 48 8B 89 30 06 00 00 48 85 C9 74 0A E8 ?? ?? ?? ?? 48 83 C4 28 C3 32 C0 48 83 C4 28 C3";
/// ChrIns: the id that check is asked with (list number and index).
const CHR_SYNC_ID: usize = 0x78;
/// The game's own check, to ask first. 0 while not hooked.
static LOCALLY_SIMULATED_ORIGINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const NO_LOCAL_NPC: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
/// Ids of the spawned characters that count, rewritten every frame. The hook runs on the
/// game's threads, so this is plain atomics and no lock.
static LOCAL_NPC_IDS: [std::sync::atomic::AtomicU32; 48] = [NO_LOCAL_NPC; 48];

extern "C" fn locally_simulated_hook(world: usize, id: u32) -> u8 {
    use std::sync::atomic::Ordering;
    let original: extern "C" fn(usize, u32) -> u8 =
        unsafe { std::mem::transmute(LOCALLY_SIMULATED_ORIGINAL.load(Ordering::Acquire)) };
    let answer = original(world, id);
    if answer != 0 {
        return answer;
    }
    // Only the mod's own part is timed: the original is the game's work.
    let id = id & 0xFFFF;
    let ours = crate::perf::time(&crate::perf::SIMULATED_HOOK, || {
        id != 0xFFFF && LOCAL_NPC_IDS.iter().any(|slot| slot.load(Ordering::Relaxed) == id)
    });
    u8::from(ours)
}

fn install_locally_simulated_hook() {
    use std::sync::atomic::Ordering;
    if LOCALLY_SIMULATED_ORIGINAL.load(Ordering::Acquire) != 0 {
        return;
    }
    let result = find_unique(LOCALLY_SIMULATED_PATTERN).and_then(|target| unsafe {
        let detour = retour::RawDetour::new(target as *const (), locally_simulated_hook as *const ())
            .map_err(|error| error.to_string())?;
        LOCALLY_SIMULATED_ORIGINAL.store(detour.trampoline() as *const () as usize, Ordering::Release);
        detour.enable().map_err(|error| error.to_string())?;
        std::mem::forget(detour);
        Ok(())
    });
    match result {
        Ok(()) => crate::log::log("spawned NPCs: simulated-here check hooked"),
        Err(error) => {
            LOCALLY_SIMULATED_ORIGINAL.store(0, Ordering::Release);
            crate::log::log(format!("spawned NPCs: simulated-here check not hooked: {error}"));
        }
    }
}

/// Rewrites the list of spawned characters that count as simulated here: all of them,
/// allies, invaders and enemies, so they can hurt each other.
fn refresh_local_npcs() {
    use std::sync::atomic::Ordering;
    if LOCALLY_SIMULATED_ORIGINAL.load(Ordering::Acquire) == 0 {
        return;
    }
    let mut ids = spawned_chrs()
        .into_iter()
        .filter_map(|chr| read_u32_checked(chr + CHR_SYNC_ID).ok())
        .map(|id| id & 0xFFFF);
    for slot in LOCAL_NPC_IDS.iter() {
        slot.store(ids.next().unwrap_or(u32::MAX), Ordering::Relaxed);
    }
}

/// The game's routine for the team a character counts as right now: `(chr) -> team`. It
/// starts from the team on the character and lets a phantom's NPC row, some special
/// effects and a list of things attached to the character override it. Hits and the AI's
/// search for targets both go by this value, not by the plain team.
const EFFECTIVE_TEAM_PATTERN: &str = "40 57 48 83 EC 40 48 C7 44 24 28 FE FF FF FF 48 89 5C 24 58 48 89 74 24 60 48 8B F9 C7 44 24 20 00 00 00 00 8B 59 74 48 83 B9 C8 11 00 00 00";
/// ChrIns: the head of the list of things attached to the character.
const CHR_ATTACHED_HEAD: usize = 0x1AE8;

/// The team the game counts a character as, its plain team, and what is attached to it
/// (kind, type word and the eight bytes after them for each entry). For the log.
fn team_report(chr: usize) -> String {
    let effective = find_unique(EFFECTIVE_TEAM_PATTERN).map(|routine| {
        let routine: extern "C" fn(usize) -> i32 = unsafe { std::mem::transmute(routine) };
        routine(chr)
    });
    let mut attached = Vec::new();
    let mut node = read_usize_checked(chr + CHR_ATTACHED_HEAD).unwrap_or(0);
    while node >= 0x10_000 && attached.len() < 24 {
        let mut bytes = [0u8; 0x20];
        if copy_readable(node, &mut bytes) != bytes.len() {
            break;
        }
        let kind = u32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let word = u16::from_ne_bytes(bytes[12..14].try_into().unwrap());
        let rest = u64::from_ne_bytes(bytes[24..32].try_into().unwrap());
        attached.push(format!("{kind:X}/{word:X}/{rest:X}"));
        node = usize::from_ne_bytes(bytes[16..24].try_into().unwrap());
    }
    format!(
        "team counted {effective:?} plain {:?} type {:?} attached [{}]",
        read_u32_checked(chr + CHR_TEAM_OFFSET),
        read_u32_checked(chr + CHR_TYPE_OFFSET),
        attached.join(" "),
    )
}

/// Where a character takes itself off the AI's list of possible targets when it goes
/// away. The instruction at +10 loads the object that holds the list: a plain vector of
/// character pointers (begin, end, end of storage). Every AI walks it each frame to find
/// enemies and friends. Whoever is not on it is only ever fought back after a hit.
const AI_TARGET_LIST_PATTERN: &str =
    "40 53 48 83 EC 20 48 8B 59 08 48 8B 0D ?? ?? ?? ?? 48 85 C9 75 26 4C 8D 0D";

/// Where the pointer to the object with the AI's target list lives. 0 until looked up.
static AI_TARGET_LIST_SLOT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// Spawned NPCs to keep on the AI's target list, by character address.
static AI_LISTED_NPCS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// How often a spawned NPC had to be put back, for a log line now and then.
static AI_LIST_ADDS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Keeps the spawned NPCs on the AI's target list. The game only lists characters a map
/// placed and drops ours again, so this checks every frame.
pub fn poll_ai_target_list() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut listed = AI_LISTED_NPCS.lock().unwrap_or_else(|poison| poison.into_inner());
        if listed.is_empty() {
            return;
        }
        if load_screen_open() {
            listed.clear();
            return;
        }
        let present = spawned_chrs();
        listed.retain(|chr| present.contains(chr));
        // One read of the whole list, so the usual frame where nothing is missing is cheap.
        let slot = AI_TARGET_LIST_SLOT.load(std::sync::atomic::Ordering::Relaxed);
        let mut entries = Vec::new();
        if slot > 1 {
            if let Ok(holder) = read_usize_checked(slot) {
                let mut bounds = [0u8; 16];
                if holder >= 0x10_000 && copy_readable(holder, &mut bounds) == bounds.len() {
                    let begin = usize::from_ne_bytes(bounds[..8].try_into().unwrap());
                    let end = usize::from_ne_bytes(bounds[8..].try_into().unwrap());
                    if begin >= 0x10_000 && end >= begin && end - begin <= 8 * 4096 {
                        let mut bytes = vec![0u8; end - begin];
                        if copy_readable(begin, &mut bytes) == bytes.len() {
                            entries = bytes.chunks_exact(8).map(|entry| usize::from_ne_bytes(entry.try_into().unwrap())).collect();
                        }
                    }
                }
            }
        }
        if !entries.is_empty() && listed.iter().all(|chr| entries.contains(chr)) {
            return;
        }
        let leaving = DELETE_REQUESTED.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
        for chr in listed.iter().copied() {
            if entries.contains(&chr) || leaving.contains(&chr) || chr_health(chr).is_none_or(|hp| hp <= 0) {
                continue;
            }
            let report = ai_target_list(chr, true);
            if report.contains("added yes") {
                let adds = AI_LIST_ADDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if adds < 6 || adds % 600 == 0 {
                    crate::log::log(format!("AI target list: put {chr:#x} back ({adds} so far): {report}"));
                }
            }
        }
    }));
}

/// Says whether a character is on the AI's list of possible targets, and with `add` puts
/// it there when it is missing, in a larger list if the old one is full. The game takes
/// it off again by itself when the character goes away.
/// Where the pointer to the object with the AI's target list lives, or 1 if it was not
/// found. Searched for once.
fn ai_target_list_slot() -> usize {
    use std::sync::atomic::Ordering;
    let mut slot = AI_TARGET_LIST_SLOT.load(Ordering::Relaxed);
    if slot == 0 {
        slot = find_unique(AI_TARGET_LIST_PATTERN)
            .and_then(|site| pattern::rip_target(site + 10, 3, 7))
            .unwrap_or(1);
        AI_TARGET_LIST_SLOT.store(slot, Ordering::Relaxed);
    }
    slot
}

fn ai_target_list(chr: usize, add: bool) -> String {
    let slot = ai_target_list_slot();
    let holder = match read_usize_checked(slot) {
        Ok(holder) if slot > 1 && holder >= 0x10_000 => holder,
        other => return format!("AI target list not found: {other:?}"),
    };
    let (Ok(begin), Ok(end), Ok(limit)) =
        (read_usize_checked(holder), read_usize_checked(holder + 8), read_usize_checked(holder + 0x10))
    else {
        return "AI target list not readable".to_string();
    };
    if begin < 0x10_000 || end < begin || limit < end || (end - begin) % 8 != 0 || end - begin > 8 * 4096 {
        return format!("AI target list looks wrong: {begin:#x} {end:#x} {limit:#x}");
    }
    let count = (end - begin) / 8;
    let listed = (0..count).any(|index| read_usize_checked(begin + index * 8) == Ok(chr));
    let player = local_player().map(|player| player as usize).unwrap_or(0);
    let player_listed = (0..count).any(|index| read_usize_checked(begin + index * 8) == Ok(player));
    let mut added = "no";
    let grow_start = Instant::now();
    if !listed && add {
        let (begin, end, limit) = if end + 8 > limit {
            // Full: move the list to storage of twice the size from the list's own allocator.
            // The old storage is left alone, a few hundred bytes at most.
            let size = end - begin;
            let moved = read_usize_checked(holder + 0x18)
                .and_then(|allocator| Ok((allocator, read_usize_checked(read_usize_checked(allocator)? + ALLOCATOR_ALLOCATE)?)))
                .ok()
                .filter(|(allocator, allocate)| *allocator >= 0x10_000 && executable_address(*allocate))
                .map(|(allocator, allocate)| {
                    let allocate: extern "C" fn(usize, usize, usize) -> usize = unsafe { std::mem::transmute(allocate) };
                    allocate(allocator, (size * 2).max(0x80), 8)
                })
                .filter(|storage| *storage >= 0x10_000);
            match moved {
                Some(storage) => {
                    let mut entries = vec![0u8; size];
                    let copied = copy_readable(begin, &mut entries) == size
                        && write_bytes_checked(storage, &entries).is_ok()
                        && write_bytes_checked(holder + 0x10, &(storage + (size * 2).max(0x80)).to_ne_bytes()).is_ok()
                        && write_bytes_checked(holder + 8, &(storage + size).to_ne_bytes()).is_ok()
                        && write_bytes_checked(holder, &storage.to_ne_bytes()).is_ok();
                    if copied { (storage, storage + size, storage + (size * 2).max(0x80)) } else { (begin, end, limit) }
                }
                None => (begin, end, limit),
            }
        } else {
            (begin, end, limit)
        };
        let _ = begin;
        added = if end + 8 > limit {
            "no room"
        } else if write_bytes_checked(end, &chr.to_ne_bytes()).is_ok()
            && write_bytes_checked(holder + 8, &(end + 8).to_ne_bytes()).is_ok()
        {
            "yes"
        } else {
            "write failed"
        };
        crate::perf::add(&crate::perf::AI_LIST_GROW, grow_start.elapsed());
    }
    format!(
        "AI target list: {count} entries, room for {}, listed {listed}, player listed {player_listed}, added {added}",
        (limit - begin) / 8
    )
}

/// A spawned NPC that gets a last step once its model and equipment have loaded.
struct LateNpcSetup {
    chr: usize,
    npc_id: u32,
    /// The phantom type, invade type and team to hand the game's phantom routine again.
    phantom: Option<(i32, u8, u32)>,
    frames: u32,
}

static LATE_NPC_SETUP: Mutex<Vec<LateNpcSetup>> = Mutex::new(Vec::new());
/// The model and its physics body are there well within this many frames.
const LATE_NPC_SETUP_FRAMES: u32 = 240;

/// Hands a built NPC to the game's own phantom routine. The routine works the team out
/// again, so the wanted team is put back afterwards. False if the game did not take it.
fn make_npc_phantom(chr: usize, npc_id: u32, kind: i32, invade_type: u8, team: u32) -> bool {
    let Some(routine) = cached().ok().and_then(|located| located.phantom_npc.as_ref().ok().copied()) else {
        crate::log::log(format!("npc {npc_id}: the game's phantom routine was not found"));
        return false;
    };
    let handle = read_u32_checked(chr + 0x08).unwrap_or(u32::MAX);
    if handle == 0 || handle == u32::MAX {
        crate::log::log(format!("npc {npc_id}: no handle for the game's phantom routine"));
        return false;
    }
    let routine: extern "C" fn(usize, u32, i32, u8) -> usize = unsafe { std::mem::transmute(routine) };
    let taken = crate::perf::time(&crate::perf::PHANTOM_ROUTINE, || routine(0, handle, kind, invade_type)) == chr;
    if taken && read_u32_checked(chr + CHR_TEAM_OFFSET) != Ok(team) {
        let _ = write_u32_checked(chr + CHR_TEAM_OFFSET, team);
    }
    if !taken {
        crate::log::log(format!("npc {npc_id}: the game's phantom routine did not take handle {handle:#x}"));
    }
    taken
}

/// The per-frame upkeep for spawned allies and invaders.
pub fn poll_spawned_npcs() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        crate::perf::time(&crate::perf::LATE_NPC_SETUP, poll_late_npc_setup);
        crate::perf::time(&crate::perf::SIMULATED_LIST, refresh_local_npcs);
        poll_phantom_list();
    }));
}

/// The session's list of phantoms: six entries from session + 0x20, 0x1C bytes each,
/// starting with the character's handle.
const PHANTOM_LIST: usize = 0x20;
const PHANTOM_ENTRY: usize = 0x1C;
const PHANTOM_ENTRIES: usize = 6;
/// Session: how many are in the world, the host included. 2 or more is multiplayer.
const SESSION_MEMBERS: usize = 0x18;

/// The session object and its phantom list as bytes.
fn phantom_list() -> Option<(usize, [u8; PHANTOM_ENTRY * PHANTOM_ENTRIES])> {
    let summon = cached().ok().and_then(|located| located.summon_npc.as_ref().ok().copied())?;
    let session: extern "C" fn() -> usize = unsafe { std::mem::transmute(summon.session) };
    let session = session();
    let mut bytes = [0u8; PHANTOM_ENTRY * PHANTOM_ENTRIES];
    (session >= 0x10_000 && copy_readable(session + PHANTOM_LIST, &mut bytes) == bytes.len()).then_some((session, bytes))
}

/// Phantom list entry: the state a phantom is in. The game's "phantom leaves" routine
/// (used when one is sent home) sets it to leaving and starts a timer; two seconds later
/// the game asks the phantom's network connection to leave and marks the timer as done.
/// A spawned NPC has no connection, so it would stay. The mod removes it at that point.
const PHANTOM_STATE: usize = 2;
const PHANTOM_TIMER: usize = 5;
const PHANTOM_LEAVING: u32 = 5;
const PHANTOM_TIMER_DONE: u32 = 0xFF7F_FFFF;
/// The game's recount of the session from its phantom list: `(session)`. It sets how many
/// white phantoms, dark spirits and members in all are in the world, and the game goes by
/// those counts to say whether the world is still in multiplayer, which locks the bonfires.
const PHANTOM_RECOUNT_PATTERN: &str = "48 89 5C 24 10 48 89 6C 24 18 48 89 74 24 20 57 48 83 EC 20 33 C0 48 8D 79 20 48 8B D9 48 8B F7 48 89 41 08 89 41 10 89 41 18 8D 68 06 0F 1F 00 83 3F FF 74 6E";

fn phantom_recount() -> Result<usize, String> {
    static ROUTINE: OnceLock<Result<usize, String>> = OnceLock::new();
    ROUTINE.get_or_init(|| find_unique(PHANTOM_RECOUNT_PATTERN)).clone()
}

/// What the game leaves in an unused entry.
const PHANTOM_EMPTY_ENTRY: [u32; PHANTOM_ENTRY / 4] = [u32::MAX, 1, 0, u32::MAX, u32::MAX, PHANTOM_TIMER_DONE, 0];

/// How long a dead ally or invader stays before it fades: a real phantom dies, falls and
/// is gone a few seconds later.
const DEAD_PHANTOM_FRAMES: u32 = 240;

/// Spawned allies and invaders that have died, by character, with frames since.
static DEAD_PHANTOMS: Mutex<Vec<(usize, u32)>> = Mutex::new(Vec::new());

/// Removes spawned allies and invaders that the game has sent home, or that died a few
/// seconds ago. The game would do this for a real phantom, but for an NPC the mod spawned
/// nothing does it, so it would stay, or lie there dead, and keep the bonfires locked.
fn poll_phantom_list() {
    let player_class = local_player().ok().and_then(|player| read_usize_checked(player as usize).ok());
    let present = spawned_chrs();
    // A character that is gone is no longer waiting to be deleted. The game reuses the
    // memory, so a new ally or invader can sit at the same address and must not count as
    // already deleted.
    DELETE_REQUESTED
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .retain(|chr| present.contains(chr));
    let spawned: Vec<(usize, u32)> = present
        .into_iter()
        .filter(|chr| player_class.is_some() && read_usize_checked(*chr).ok() == player_class)
        .filter_map(|chr| read_u32_checked(chr + 0x08).ok().map(|handle| (chr, handle)))
        .collect();
    let mut dead = DEAD_PHANTOMS.lock().unwrap_or_else(|poison| poison.into_inner());
    dead.retain(|(chr, _)| spawned.iter().any(|(present, _)| present == chr));
    if spawned.is_empty() {
        return;
    }
    let mut leaving: Vec<(usize, u32, &str)> = Vec::new();
    for &(chr, handle) in &spawned {
        if chr_health(chr).is_none_or(|hp| hp > 0) {
            dead.retain(|(gone, _)| *gone != chr);
            continue;
        }
        match dead.iter_mut().find(|(gone, _)| *gone == chr) {
            Some((_, frames)) => {
                *frames += 1;
                if *frames == DEAD_PHANTOM_FRAMES {
                    leaving.push((chr, handle, "died"));
                }
            }
            None => dead.push((chr, 0)),
        }
    }
    drop(dead);
    let list = phantom_list();
    if let Some((_, bytes)) = list {
        for index in 0..PHANTOM_ENTRIES {
            let words: Vec<u32> = bytes[index * PHANTOM_ENTRY..(index + 1) * PHANTOM_ENTRY]
                .chunks_exact(4)
                .map(|word| u32::from_ne_bytes(word.try_into().unwrap()))
                .collect();
            if let Some(&(chr, handle)) = spawned.iter().find(|(_, handle)| *handle == words[0]) {
                if words[PHANTOM_STATE] == PHANTOM_LEAVING
                    && words[PHANTOM_TIMER] == PHANTOM_TIMER_DONE
                    && !leaving.iter().any(|(gone, _, _)| *gone == chr)
                {
                    leaving.push((chr, handle, "was sent home"));
                }
            }
        }
    }
    for (chr, handle, why) in leaving {
        remove_phantom(chr, handle, why);
    }
}

/// Removes one spawned ally or invader. Deleting it also clears its place in the phantom
/// list (see `forget_phantom_entry`).
fn remove_phantom(chr: usize, handle: u32, why: &str) {
    let mut requested = DELETE_REQUESTED.lock().unwrap_or_else(|poison| poison.into_inner());
    if requested.contains(&chr) {
        return;
    }
    let removed = delete_spawns(&[chr], &mut requested);
    drop(requested);
    crate::log::log(format!("phantom {handle:#x} {why}: removed {removed:?}"));
}

/// Clears a deleted character's place in the session's phantom list, and recounts the
/// session. Without the recount the game still thinks someone is visiting, which keeps
/// the bonfires locked.
fn forget_phantom_entry(handle: u32) {
    let Some((session, bytes)) = phantom_list() else {
        return;
    };
    let empty: Vec<u8> = PHANTOM_EMPTY_ENTRY.iter().flat_map(|word| word.to_ne_bytes()).collect();
    let mut cleared = Vec::new();
    for index in 0..PHANTOM_ENTRIES {
        let first = u32::from_ne_bytes(bytes[index * PHANTOM_ENTRY..index * PHANTOM_ENTRY + 4].try_into().unwrap());
        if first == handle {
            cleared.push((index, write_bytes_checked(session + PHANTOM_LIST + index * PHANTOM_ENTRY, &empty).is_ok()));
        }
    }
    if cleared.is_empty() {
        return;
    }
    let members = phantom_recount().map(|routine| {
        let recount: extern "C" fn(usize) = unsafe { std::mem::transmute(routine) };
        recount(session);
        read_u32_checked(session + SESSION_MEMBERS)
    });
    crate::log::log(format!("phantom {handle:#x}: list places cleared {cleared:?}, members now {members:?}"));
}

/// Runs the last step for spawned NPCs whose model and equipment have had time to load.
fn poll_late_npc_setup() {
    let mut waiting = LATE_NPC_SETUP.lock().unwrap_or_else(|poison| poison.into_inner());
    if waiting.is_empty() {
        return;
    }
    if load_screen_open() || unsafe { WorldChrMan::instance() }.is_err() {
        waiting.clear();
        return;
    }
    for setup in waiting.iter_mut() {
        setup.frames += 1;
    }
    let due: Vec<LateNpcSetup> = {
        let (due, later): (Vec<_>, Vec<_>) =
            waiting.drain(..).partition(|setup| setup.frames >= LATE_NPC_SETUP_FRAMES);
        *waiting = later;
        due
    };
    drop(waiting);
    let present = spawned_chrs();
    for setup in due.into_iter().filter(|setup| present.contains(&setup.chr)) {
        let LateNpcSetup { chr, npc_id, phantom, .. } = setup;
        // Once more now that the model and the equipment are loaded: the game only ever
        // runs this on an NPC that has been standing in the map for a while.
        if let Some((kind, invade_type, team)) = phantom {
            make_npc_phantom(chr, npc_id, kind, invade_type, team);
        }
        // Map characters are on the list every AI picks its targets from. Spawned ones are
        // not, so without this, allies and invaders only fight what hits them first.
        crate::log::log(format!("npc {npc_id}: {}", ai_target_list(chr, true)));
        let mut listed = AI_LISTED_NPCS.lock().unwrap_or_else(|poison| poison.into_inner());
        if !listed.contains(&chr) {
            listed.push(chr);
        }
    }
}

/// Invaders that carry a sender's name instead of their own, by character address.
static INVADER_NAMES: Mutex<Vec<(usize, Vec<u16>)>> = Mutex::new(Vec::new());
/// The game's own name routine, to call for every other character. 0 while not hooked.
static CHR_NAME_ORIGINAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn forget_invader_name(chr: usize) {
    let mut names = INVADER_NAMES.lock().unwrap_or_else(|poison| poison.into_inner());
    names.retain(|(named, _)| *named != chr);
}

/// A death, a bonfire rest or a warp reloads the area and removes every spawned character,
/// so all names go with them. Runs every frame; it only looks at the game while names exist.
fn forget_invader_names_on_reload() {
    let mut names = INVADER_NAMES.lock().unwrap_or_else(|poison| poison.into_inner());
    if !names.is_empty() && (load_screen_open() || unsafe { WorldChrMan::instance() }.is_err()) {
        names.clear();
    }
}

/// The name to show for `chr`, if it is one of our named invaders and still exists. A
/// deleted invader's address can be reused by another character, so a name is dropped
/// when its invader is deleted, when the area reloads, when a new spawn takes the address,
/// and here if the character is simply gone.
fn invader_name(chr: usize) -> Option<Vec<u16>> {
    let mut names = INVADER_NAMES.lock().unwrap_or_else(|poison| poison.into_inner());
    if names.is_empty() {
        return None;
    }
    let index = names.iter().position(|(named, _)| *named == chr)?;
    if !spawned_chrs().contains(&chr) {
        names.remove(index);
        return None;
    }
    Some(names[index].1.clone())
}

/// Stands in for the game's name routine. Our named invaders get the sender's name, and
/// every other character goes to the original.
extern "C" fn chr_name_hook(out: usize, chr: usize, decorated: u8) -> usize {
    use std::sync::atomic::Ordering;
    let original: extern "C" fn(usize, usize, u8) -> usize =
        unsafe { std::mem::transmute(CHR_NAME_ORIGINAL.load(Ordering::Acquire)) };
    // Only the mod's own part is timed: the original is the game's work.
    let named = crate::perf::time(&crate::perf::NAME_HOOK, || {
        chr >= 0x10_000
            && out >= 0x10_000
            && matches!(
                catch_unwind(AssertUnwindSafe(|| {
                    invader_name(chr).is_some_and(|units| fill_name_object(out, &units).is_ok())
                })),
                Ok(true)
            )
    });
    if named {
        return out;
    }
    original(out, chr, decorated)
}

fn install_chr_name_hook() {
    use std::sync::atomic::Ordering;
    if CHR_NAME_ORIGINAL.load(Ordering::Acquire) != 0 {
        return;
    }
    let result = find_unique(CHR_NAME_PATTERN).and_then(|target| unsafe {
        let detour = retour::RawDetour::new(target as *const (), chr_name_hook as *const ())
            .map_err(|error| error.to_string())?;
        // The original must be known before the first redirected call arrives.
        CHR_NAME_ORIGINAL.store(detour.trampoline() as *const () as usize, Ordering::Release);
        detour.enable().map_err(|error| error.to_string())?;
        // Stays in place for as long as the game runs.
        std::mem::forget(detour);
        Ok(())
    });
    match result {
        Ok(()) => crate::log::log("invader names: name routine hooked"),
        Err(error) => {
            CHR_NAME_ORIGINAL.store(0, Ordering::Release);
            crate::log::log(format!("invader names: not available, invaders keep their own name: {error}"));
        }
    }
}

/// Phantom type and side, the sender's name if wanted, and the arrival animation. The gear,
/// health and AI already came with the character, and the message went out before.
fn finish_invader(chr: usize, npc_id: u32, note: Option<InvaderNote>, friendly: bool) {
    let kind = read_u32_checked(chr + CHR_TYPE_OFFSET).unwrap_or(0) as i32;
    let team = read_u32_checked(chr + CHR_TEAM_OFFSET).unwrap_or(0);
    // An invader is a dark spirit on the invaders' side. An ally is a white phantom and keeps
    // the side the game gives its summon NPCs.
    let (new_kind, new_team) =
        if friendly { (CHR_TYPE_WHITE_PHANTOM, team) } else { (CHR_TYPE_DARK_SPIRIT, CHR_TEAM_DARK_SPIRIT) };
    // The game's own routine sets the type and the team and marks the player data as a
    // phantom's, the way sign summons and scripted invasions do.
    let made = make_npc_phantom(chr, npc_id, new_kind, 0, new_team);
    let summon = cached().ok().and_then(|located| located.summon_npc.as_ref().ok().copied());
    if !made && new_kind != kind {
        match summon {
            // The game's own setter also refreshes what follows from the type.
            Some(summon) => {
                let set_type: extern "C" fn(usize, i32) = unsafe { std::mem::transmute(summon.set_type) };
                set_type(chr, new_kind);
            }
            None => {
                if let Err(error) = write_i32_checked(chr + CHR_TYPE_OFFSET, new_kind) {
                    crate::log::log(format!("npc {npc_id}: chr type not set: {error}"));
                }
            }
        }
    }
    // The session's phantom list puts the NPC in the list on the left of the screen. An
    // ally joins the way a summoned NPC does, an invader as already arrived, like the
    // game's scripted invaders. Neither makes the game show a message of its own.
    match summon {
        Some(summon) => {
            let session: extern "C" fn() -> usize = unsafe { std::mem::transmute(summon.session) };
            let join: extern "C" fn(usize, u32, i32, u32, i32, i32, i32) = unsafe { std::mem::transmute(summon.join) };
            let list = session();
            let handle = read_u32_checked(chr + 0x08).unwrap_or(0);
            if list >= 0x10_000 && handle != 0 {
                let stage = if friendly { 1 } else { 2 };
                join(list, handle, new_kind, stage, -1, if friendly { -1 } else { 0 }, 0);
            } else {
                crate::log::log(format!("npc {npc_id}: no session to join"));
            }
        }
        None => crate::log::log(format!("npc {npc_id}: the summon routine was not found")),
    }
    if read_u32_checked(chr + CHR_TEAM_OFFSET) != Ok(new_team) {
        if let Err(error) = write_u32_checked(chr + CHR_TEAM_OFFSET, new_team) {
            crate::log::log(format!("npc {npc_id}: team not set: {error}"));
        }
    }
    // Without this the game counts spawned characters as another player's, and their hits
    // only land on the local player.
    install_locally_simulated_hook();
    refresh_local_npcs();
    LATE_NPC_SETUP
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .push(LateNpcSetup { chr, npc_id, phantom: made.then_some((new_kind, 0, new_team)), frames: 0 });
    let sender = note.as_ref().and_then(|note| note.name.as_deref());
    if let Some(name) = sender {
        let units = name_units(name, MESSAGE_NAME_LENGTH);
        if !units.is_empty() && CHR_NAME_ORIGINAL.load(std::sync::atomic::Ordering::Acquire) != 0 {
            let mut names = INVADER_NAMES.lock().unwrap_or_else(|poison| poison.into_inner());
            names.retain(|(named, _)| *named != chr);
            names.push((chr, units));
        }
    }
    let animation = note.as_ref().map(|note| note.animation).unwrap_or(0);
    if animation > 0 {
        let played = module_pointer(chr, EVENT_MODULE_OFFSET)
            .and_then(|event| write_i32_checked(event + IDLE_ANIM_OFFSET, animation));
        if let Err(error) = played {
            crate::log::log(format!("invader {npc_id}: arrival animation not played: {error}"));
        }
    }
    crate::log::log(format!(
        "{} {npc_id}: built at {chr:#x}, type {kind} -> {new_kind}, team {team} -> {new_team}, hp {:?}, name {:?}, animation {animation}",
        if friendly { "ally" } else { "invader" },
        chr_health(chr),
        sender.or_else(|| known_invader_name(npc_id)),
    ));
}

pub struct InvaderOptions<'a> {
    /// Whoever sent it. Shown on the invader and in the message when `use_sender_name` is on.
    pub viewer: &'a str,
    pub use_sender_name: bool,
    pub ids_text: &'a str,
    /// Above 0 picks the AI row by hand.
    pub think_override: u32,
    /// Seconds between the invasion message and the invader appearing.
    pub arrival_delay: u32,
    /// The animation the invader appears with, or 0 for none.
    pub arrival_animation: i32,
    /// Arrives as a white phantom on the player's side instead of a dark spirit.
    pub friendly: bool,
}

impl<'a> InvaderOptions<'a> {
    /// A random invader with Spawn NPC Invader's default settings.
    pub fn invader(viewer: &'a str) -> Self {
        Self {
            viewer,
            use_sender_name: true,
            ids_text: "",
            think_override: 0,
            arrival_delay: 4,
            arrival_animation: 63_010,
            friendly: false,
        }
    }
}

/// True with the given chance in percent.
pub fn roll_percent(percent: u32) -> bool {
    (random_index(100) as u32) < percent
}

/// Queues one random named NPC invader near you.
pub fn spawn_npc_invader(options: InvaderOptions) -> Result<String, String> {
    let InvaderOptions {
        viewer,
        use_sender_name,
        ids_text,
        think_override,
        arrival_delay,
        arrival_animation,
        friendly,
    } = options;
    let routine = create_npc_address()?;
    let creator = debug_creator()?;
    let located = cached()?;
    located.map_chr_factory.as_ref().map_err(|error| error.clone())?;
    located.map_chr_place.as_ref().map_err(|error| error.clone())?;
    let regulation = unsafe { CSRegulationManager::instance() }
        .map_err(|error| format!("the NPC list is unavailable: {error:?}"))?;
    let npcs = regulation.get_param::<NPC_PARAM_ST>();
    let thinks = regulation.get_param::<NPC_THINK_PARAM_ST>();
    let mut think_ids: Vec<u32> = thinks.iter().map(|(id, _)| id as u32).filter(|id| *id > 0).collect();
    think_ids.sort_unstable();

    let (ashes, ringed_city) = unsafe { darksouls3::cs::CSDlc::instance() }
        .map(|dlc| (dlc.dlc1_installed, dlc.dlc2_installed))
        .unwrap_or((false, false));
    let sides = natural_sides();
    let mut present: Vec<u32> = if friendly {
        sides
            .iter()
            .filter(|(_, side)| is_summon_side(side.team, side.phantom))
            .map(|(id, _)| *id)
            .collect()
    } else {
        invader_candidates(ashes, ringed_city)
            .iter()
            .map(|(id, _)| *id)
            .filter(|id| npcs.get(u64::from(*id)).is_some())
            .collect()
    };
    present.sort_unstable();
    if friendly {
        static LISTED: std::sync::Once = std::sync::Once::new();
        LISTED.call_once(|| crate::log::log(format!("ally: the game's summon NPCs are {present:?}")));
    }
    let requested = parse_npc_param_ids(ids_text);
    let choices = choose_invader_ids(&requested, &present);
    if choices.is_empty() {
        return Err(if requested.is_empty() {
            "no such NPCs were found in the game data".to_string()
        } else if friendly {
            "none of those NPC ids are summons in the game data".to_string()
        } else {
            "none of those NPC ids are available as invaders".to_string()
        });
    }

    let npc_id = choices[random_index(choices.len())];
    let row = npcs
        .get(u64::from(npc_id))
        .ok_or_else(|| format!("NPC {npc_id} disappeared from the game data"))?;
    let init_think = invader_init_think(npc_id)
        .ok_or_else(|| format!("NPC {npc_id} has no CharaInit row, so it cannot be built"))?;
    let think_id = if think_override > 0 {
        think_override
    } else {
        think_for_invader(npc_id, row.ai_think_id(), init_think, &think_ids)
            .ok_or_else(|| format!("NPC {npc_id} has no usable AI"))?
    };
    let pick = EnemyChoice {
        npc_id,
        think_id,
        invader: true,
        label: keep_invader_note(
            use_sender_name.then_some(viewer),
            Duration::from_secs(u64::from(arrival_delay)),
            arrival_animation,
            row.name_id(),
        ),
        friendly,
    };
    let mut slot = ENEMY_QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
    match slot.as_mut() {
        Some(queue) => {
            queue.remaining.push(pick);
            queue.pool.push(pick);
            queue.replacements = 0;
        }
        None => {
            *slot = Some(EnemyQueue {
                creator,
                routine,
                remaining: vec![pick],
                pool: vec![pick],
                replace_oldest: true,
                limit: 5,
                full_frames: 0,
                pause_frames: 0,
                replacements: 0,
                checks: Vec::new(),
                in_flight: None,
                started: 0,
            });
        }
    }

    let name = known_invader_name(npc_id).unwrap_or("NPC invader");
    let viewer = viewer.trim();
    if viewer.is_empty() || !use_sender_name {
        Ok(format!("spawning {name} ({npc_id}, AI {think_id})"))
    } else {
        Ok(format!("spawning {name} ({npc_id}, AI {think_id}) as {viewer}"))
    }
}

pub fn poll_enemy_queue() {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        forget_invader_names_on_reload();
        let mut slot = ENEMY_QUEUE.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(queue) = slot.as_mut() else {
            return;
        };
        // The world goes away before the load screen shows, and the creator needs it.
        if load_screen_open() || unsafe { WorldChrMan::instance() }.is_err() {
            slot.take();
            // Invaders that were still waiting are gone with the queue.
            INVADER_NOTES.lock().unwrap_or_else(|poison| poison.into_inner()).clear();
            return;
        }
        poll_spawn_checks(queue);
        let finished = queue.in_flight.as_mut().and_then(|flight| {
            flight.frames += 1;
            let chr = read_usize_checked(queue.creator + CREATOR_RESULT).unwrap_or(0);
            if chr >= 0x10_000 && chr != flight.previous {
                Some((chr, flight.npc_id))
            } else {
                None
            }
        });
        if let Some((chr, npc_id)) = finished {
            queue.in_flight = None;
            if npc_id == MIMIC {
                let mut made = NEW_MIMICS.lock().unwrap_or_else(|poison| poison.into_inner());
                made.push(chr);
                // Nobody collects them outside Mimic Chests, so only the latest few stay.
                let extra = made.len().saturating_sub(MIMIC_ROW.len());
                made.drain(..extra);
            }
            // A new enemy can land on the address of an invader that is gone.
            forget_invader_name(chr);
            queue.checks.push(SpawnCheck {
                chr,
                frames: 0,
                skip_frozen_delete: false,
            });
            return;
        }
        if let Some(flight) = queue.in_flight.as_mut() {
            if flight.frames >= SPAWN_WAIT_FRAMES {
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
        if enemy.invader && !invader_arrival_due(enemy) {
            queue.remaining.push(enemy);
            return;
        }
        if enemy.invader {
            match crate::perf::time(&crate::perf::NPC_BUILD, || spawn_human_npc(queue, enemy)) {
                Ok(chr) => {
                    queue.started += 1;
                    let note = take_invader_note(enemy.label);
                    crate::perf::time(&crate::perf::NPC_FINISH, || finish_invader(chr, enemy.npc_id, note, enemy.friendly));
                    queue.checks.push(SpawnCheck {
                        chr,
                        frames: 0,
                        skip_frozen_delete: true,
                    });
                }
                Err(error) => {
                    take_invader_note(enemy.label);
                    crate::log::log(format!("invader {}: not spawned: {error}", enemy.npc_id));
                }
            }
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
                { "name": "delta", "type": "int", "label": "Souls to add", "hint": "A negative number takes souls away, for example -100.", "default": -100, "min": -999999999, "max": 999999999 },
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
            "arguments": [{ "name": "seconds", "type": "int", "default": 10, "min": 1, "max": 60 }]
        },
        {
            "id": "decrease_speed",
            "name": "Decrease Speed",
            "description": "Slows the player by a percentage for the chosen time. Adds together with Increase Speed.",
            "arguments": [
                { "name": "percent", "type": "int", "default": 30, "min": 1, "max": 90 },
                { "name": "seconds", "type": "int", "default": 10, "min": 1, "max": 300 },
                { "name": "movementOnly", "type": "bool", "label": "Movement only (experimental)", "hint": "Only walking, running and rolling change speed. Attacks can still start at the changed speed, because the game reports them a moment late.", "default": 0, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "increase_speed",
            "name": "Increase Speed",
            "description": "Speeds up the player by a percentage for the chosen time. Adds together with Decrease Speed.",
            "arguments": [
                { "name": "percent", "type": "int", "default": 30, "min": 1, "max": 300 },
                { "name": "seconds", "type": "int", "default": 10, "min": 1, "max": 300 },
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
            "id": "unequip_weapon",
            "name": "Unequip Weapon",
            "description": "Takes the weapon you are holding out of your hand. It stays in your inventory, so you can equip it again from the menu.",
            "arguments": [
                { "name": "hand", "type": "choice", "label": "Hand", "options": ["Right hand", "Left hand", "Either"], "default": 0, "min": 0, "max": 2 }
            ]
        },
        {
            "id": "delete_weapon",
            "name": "Delete Weapon",
            "description": "Deletes the weapon you are holding from your inventory for good. A gear snapshot on the General page can bring it back.",
            "arguments": [
                { "name": "hand", "type": "choice", "label": "Hand", "options": ["Right hand", "Left hand", "Either"], "default": 0, "min": 0, "max": 2 }
            ]
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
                { "name": "seconds", "type": "int", "default": 120, "min": 1, "max": 600 },
                { "name": "armorStyle", "type": "choice", "label": "Armor style", "hint": "Matching set picks one armor set. If it has no piece for a slot, that slot stays as it is.", "options": ["Random pieces", "Matching set", "Either at random"], "default": 0, "min": 0, "max": 2 },
                { "name": "includeMissingDlc", "type": "bool", "label": "Include armor from DLC you do not have (experimental)", "hint": "Weapons from a missing DLC worked in a short test, but armor has not been tried.", "default": 0, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "bombardier",
            "name": "Bombardier",
            "description": "Takes away all your weapons, dresses you in the Mirrah Chain Set and gives you endless Black Firebombs in a free quick slot, then puts everything back when the time is up.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 120, "min": 5, "max": 600 }]
        },
        {
            "id": "firestarter",
            "name": "Firestarter",
            "description": "Takes away your weapons and gives you a Pyromancy Flame at a random level, the Fire Witch Set and every pyromancy. As many as your slots hold are attuned, without stat requirements. Everything goes back when the time is up.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 120, "min": 5, "max": 600 }]
        },
        {
            "id": "test_mimic_chests",
            "name": "Test Mimic chests",
            "description": "Developer test: spawns three Mimics in a row in front of you, and for 90 seconds logs your animation and that of everything within 6 m, map enemies included.",
            "arguments": [
                { "name": "spawn", "type": "bool", "label": "Spawn the Mimics", "hint": "Off only logs, for example next to a real Mimic while you open it.", "default": 1, "min": 0, "max": 1 },
                { "name": "npcId", "type": "int", "label": "NPC id", "hint": "Which Mimic row to spawn. 212000 is the High Wall Mimic.", "default": 212000, "min": 100000, "max": 999999 },
                { "name": "thinkId", "type": "int", "label": "AI row", "hint": "0 uses the NPC's own AI.", "default": 0, "min": 0, "max": 99999999 }
            ]
        },
        {
            "id": "play_nearest_sequence",
            "name": "Play nearest animation chain",
            "description": "Developer test: the character closest to you plays the listed animations one after another.",
            "arguments": [
                { "name": "animations", "type": "string", "label": "Animation ids", "hint": "Comma-separated, played in this order.", "defaultText": "3000, 3004, 4100" },
                { "name": "gapFrames", "type": "int", "label": "Frames between them", "hint": "60 frames are about a second.", "default": 45, "min": 21, "max": 600 }
            ]
        },
        {
            "id": "try_vanish_effect",
            "name": "Try vanish effect",
            "description": "Developer test: the closest spawned enemy gets the chosen special effect and is removed a moment later.",
            "arguments": [
                { "name": "effect", "type": "int", "label": "Effect id", "hint": "10633, 10635 and 12030 are transparency effects; 101341000 is Hidden Body; 0 for none.", "default": 10633, "min": 0, "max": 999999999 },
                { "name": "seconds", "type": "int", "label": "Seconds until removed", "default": 2, "min": 0, "max": 30 }
            ]
        },
        {
            "id": "scan_nearest_animations",
            "name": "Scan nearest animations",
            "description": "Developer test: tries a list of animation ids on the character closest to you, one every few frames, and logs which ones it really plays. Takes about 30 seconds.",
            "arguments": []
        },
        {
            "id": "force_nearest_animation",
            "name": "Force nearest animation",
            "description": "Developer test: the character closest to you, within 6 m, plays the chosen animation.",
            "arguments": [
                { "name": "animation", "type": "int", "label": "Animation id", "default": 2003010, "min": 0, "max": 99999999 },
                { "name": "playerAnimation", "type": "int", "label": "Player animation id", "hint": "Optional. Above 0, you play this one at the same moment.", "default": 0, "min": 0, "max": 99999999 }
            ]
        },
        {
            "id": "mimic_chests",
            "name": "Mimic Chests",
            "description": "Three chests appear in front of you. One holds a good weapon or ring, fully upgraded; the other two are Mimics. Open one with the interact button. A wrong pick bites you and the other Mimic wakes up. Attacking a chest, or taking too long, wakes all three and wins nothing.",
            "arguments": [
                { "name": "seconds", "type": "int", "label": "Seconds to pick", "default": 30, "min": 10, "max": 600 },
                { "name": "prize", "type": "choice", "label": "Prize", "options": ["Weapon or ring", "Weapon", "Ring"], "default": 0, "min": 0, "max": 2 }
            ]
        },
        {
            "id": "ooga_booga",
            "name": "Ooga Booga",
            "description": "Takes away your weapons and armor and gives you a Club at its highest level, without stat requirements and with War Cry's attack boost, then puts everything back when the time is up.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 120, "min": 5, "max": 600 }]
        },
        {
            "id": "big_ooga_booga",
            "name": "Big Ooga Booga",
            "description": "Takes away your weapons and armor and gives you a Great Club at its highest level, without stat requirements and with War Cry's attack boost, then puts everything back when the time is up.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 120, "min": 5, "max": 600 }]
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
                { "name": "seconds", "type": "int", "default": 30, "min": 1, "max": 300 },
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
            "arguments": [{ "name": "seconds", "type": "int", "default": 10, "min": 1, "max": 60 }]
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
            "id": "pacifist",
            "name": "Pacifist",
            "description": "No attacking, blocking or using items for the chosen number of seconds. Use selected item still works. Controller only.",
            "arguments": [{ "name": "seconds", "type": "int", "default": 30, "min": 1, "max": 300 }]
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
            "description": "Spawns random regular enemies near you, from the current map unless Whole game is on.",
            "arguments": [
                { "name": "amount", "type": "int", "default": 1, "min": 1, "max": 10 },
                { "name": "limit", "type": "int", "label": "Enemy limit", "hint": "How many spawned enemies can be active at once. The game itself stops at 16.", "default": 5, "min": 1, "max": 16 },
                { "name": "wholeGame", "type": "bool", "label": "Whole game (experimental)", "hint": "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly.", "default": 0, "min": 0, "max": 1 },
                { "name": "replaceOldest", "type": "bool", "label": "Replace oldest at limit", "hint": "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot.", "default": 1, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "spawn_random_boss",
            "name": "Spawn Random Boss",
            "description": "Spawns a random boss near you, from the current map unless Whole game is on. Many bosses rely on map events, so some may act oddly.",
            "arguments": [
                { "name": "amount", "type": "int", "default": 1, "min": 1, "max": 3 },
                { "name": "limit", "type": "int", "label": "Enemy limit", "hint": "How many spawned enemies can be active at once. The game itself stops at 16.", "default": 5, "min": 1, "max": 16 },
                { "name": "wholeGame", "type": "bool", "label": "Whole game (experimental)", "hint": "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly.", "default": 0, "min": 0, "max": 1 },
                { "name": "replaceOldest", "type": "bool", "label": "Replace oldest at limit", "hint": "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot.", "default": 1, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "spawn_anything",
            "name": "Spawn Anything",
            "description": "Spawns one random thing near you: any enemy or boss, or sometimes an NPC invader.",
            "arguments": [
                { "name": "invaderChance", "type": "int", "label": "NPC invader chance", "hint": "The chance in percent that an NPC invader comes instead of an enemy. Invaders use the Spawn NPC Invader defaults.", "default": 20, "min": 0, "max": 100 },
                { "name": "limit", "type": "int", "label": "Enemy limit", "hint": "How many spawned enemies can be active at once. The game itself stops at 16.", "default": 5, "min": 1, "max": 16 },
                { "name": "wholeGame", "type": "bool", "label": "Whole game (experimental)", "hint": "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly.", "default": 0, "min": 0, "max": 1 },
                { "name": "replaceOldest", "type": "bool", "label": "Replace oldest at limit", "hint": "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot.", "default": 1, "min": 0, "max": 1 }
            ]
        },
        {
            "id": "spawn_npc_ally",
            "name": "Spawn NPC Ally",
            "description": "Summons one of the game's own summonable NPCs near you as a white phantom that fights on your side. The game announces the summon, with the sender's name if you want.",
            "arguments": [
                { "name": "npcParamIds", "type": "string", "label": "NPC ids", "hint": "Optional. Comma-separated NpcParam ids of summon NPCs. Empty picks a random one.", "defaultText": "" },
                { "name": "useSenderName", "type": "bool", "label": "Show the sender's name on the phantom", "hint": "On: the summon message and the name on lock-on show whoever triggered it. Off: both show the NPC's own name.", "default": 1, "min": 0, "max": 1 },
                { "name": "arrivalDelay", "type": "int", "label": "Seconds between message and arrival", "hint": "The summon message shows first, and the phantom appears this many seconds later.", "default": 4, "min": 0, "max": 30 },
                { "name": "arrivalAnimation", "type": "int", "label": "Arrival animation", "hint": "The animation the phantom appears with. 63020 is the gold-sign summon pose, 63010 the hunched stand-up of an invader, 63000 a stand-up from a crouch. 0 for none.", "default": 63020, "min": 0, "max": 99999 },
                { "name": "thinkParamId", "type": "int", "label": "AI row", "hint": "Optional. 0 picks the NPC's own AI. Another NpcThinkParam id forces that AI.", "default": 0, "min": 0, "max": 99999999 }
            ]
        },
        {
            "id": "spawn_npc_invader",
            "name": "Spawn NPC Invader",
            "description": "Spawns a random named NPC invader near you as a Dark Spirit, with its own gear (Havel, Heysel, Hodrick and the rest of the known set). The game announces the invasion, with the sender's name if you want.",
            "arguments": [
                { "name": "npcParamIds", "type": "string", "label": "NPC ids", "hint": "Optional. Comma-separated NpcParam ids. Empty picks a random known invader.", "defaultText": "" },
                { "name": "useSenderName", "type": "bool", "label": "Show the sender's name on the invader", "hint": "On: the invasion message and the name on lock-on show whoever triggered it. Off: both show the NPC's own name.", "default": 1, "min": 0, "max": 1 },
                { "name": "arrivalDelay", "type": "int", "label": "Seconds between message and arrival", "hint": "The invasion message shows first, and the invader appears this many seconds later.", "default": 4, "min": 0, "max": 30 },
                { "name": "arrivalAnimation", "type": "int", "label": "Arrival animation", "hint": "The animation the invader appears with. 63010 is the hunched stand-up of an invader, 63020 the gold-sign summon pose, 63000 a stand-up from a crouch. 0 for none.", "default": 63010, "min": 0, "max": 99999 },
                { "name": "thinkParamId", "type": "int", "label": "AI row", "hint": "Optional. 0 picks the invader's own AI. Another NpcThinkParam id forces that AI.", "default": 0, "min": 0, "max": 99999999 }
            ]
        },
        {
            "id": "dump_player_data",
            "name": "Dump player data",
            "description": "Writes the local player data block to a file, including the reported soul count.",
            "arguments": [{ "name": "soulCount", "type": "int", "default": 0, "min": 0, "max": 999999999 }]
        },
        {
            "id": "give_ember",
            "name": "Give Ember",
            "description": "Gives you Embers. Use one to become embered, which is needed to summon phantoms.",
            "arguments": [{ "name": "amount", "type": "int", "default": 1, "min": 1, "max": 99 }]
        },
        {
            "id": "toggle_invincible",
            "name": "Toggle invincibility",
            "description": "For testing: no hit damage until you trigger it again. It stays on through deaths, rests and warps.",
            "arguments": []
        },
        {
            "id": "toggle_invisible",
            "name": "Toggle invisibility",
            "description": "For testing: enemies neither see nor hear you until you trigger it again.",
            "arguments": []
        },
        {
            "id": "dump_phantoms",
            "name": "Dump phantoms",
            "description": "Writes the state of every player-type character in the area to a file (you, real NPC phantoms and spawned ones), and of the enemies within 40 metres of you.",
            "arguments": []
        },
        {
            "id": "toggle_performance",
            "name": "Toggle performance measuring",
            "description": "For testing: starts measuring how smoothly the game runs and how much time the mod takes. Trigger it again to stop; that also writes a last report.",
            "arguments": []
        },
        {
            "id": "performance_report",
            "name": "Performance report",
            "description": "While performance measuring is on: shows how smoothly the game ran since the last report, how much time the mod used, whether the mod was busy in any slow frame, and how busy the game's threads were. The full report goes to ds3mod.log.",
            "arguments": []
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
        apply_soul_delta, bombs_to_remove, gear_snapshot_due, GearSnapshot, GearSnapshots, GEAR_SNAPSHOT_EVERY, can_restore_stats, is_estus, next_level_cost, pick_bomb_slot,
        shuffled_stats,
        ArmorSwap, Bombardier, PendingRestores, RemovedWeapon, ShuffledStats, TemporaryArmor, soul_delta_for_level, armor_available, armor_sets, can_restore_armor, classify_slot, ArmorStyle, combined_speed, is_real_armor, health_at_percent, is_base_weapon_id,
        is_empty_item, is_listed_armor, is_movement_animation, is_removable_weapon, pick_infusion,
        pick_level,
        ai_choices, enemy_teams, has_fight_logic, is_spawnable_enemy, most_common_team, spawn_state,
        SpawnState, model_from_chr_name, think_candidates, usable_two_handed, wait_reason, weapon_available,
        weapon_infusion, weapon_level, WeaponTrait, with_no_damage_bit, without_bits,
        attunement_slots, magic_entry, pick_attunement, pyromancies_available, ClubRow, EndedKind, EndedSwap, Firestarter,
        OogaBooga,
        SpellRequirement,
        ended_prefix, choose_invader_ids, known_invader_name, parse_npc_param_ids, think_for_invader,
        clear_entity_ids, is_summon_side, name_units, MESSAGE_NAME_LENGTH,
    };

    #[test]
    fn npc_id_list_uses_the_built_in_invaders_when_empty() {
        let present = [27_420, 27_700, 29_540];
        assert_eq!(choose_invader_ids(&[], &present), present);
        assert_eq!(choose_invader_ids(&[27_700, 1], &present), vec![27_700]);
        assert!(choose_invader_ids(&[1, 2], &present).is_empty());
        assert_eq!(parse_npc_param_ids("27420, 27700\n29540"), vec![27_420, 27_700, 29_540]);
        assert_eq!(known_invader_name(27_700), Some("Holy Knight Hodrick"));
        assert_eq!(known_invader_name(1), None);
    }

    #[test]
    fn invader_ai_prefers_charainit_think_then_npc_param() {
        let thinks = [100u32, 200, 300];
        assert_eq!(think_for_invader(27_700, 200, 100, &thinks), Some(100));
        assert_eq!(think_for_invader(27_700, 200, 0, &thinks), Some(200));
        assert_eq!(think_for_invader(27_700, 0, 0, &thinks), None);
    }

    #[test]
    fn a_viewer_name_is_cut_to_fit_without_splitting_a_character() {
        assert_eq!(name_units("Viewer", MESSAGE_NAME_LENGTH), "Viewer".encode_utf16().collect::<Vec<u16>>());
        assert_eq!(name_units("a_very_long_twitch_name", 16).len(), 16);
        assert_eq!(name_units("a_very_long_twitch_name", MESSAGE_NAME_LENGTH).len(), 23);
        // An emoji takes two units. Fifteen letters leave room for one unit only, so it is dropped.
        assert_eq!(name_units("abcdefghijklmno\u{1F525}", 16).len(), 15);
        assert_eq!(name_units("line\nbreak", MESSAGE_NAME_LENGTH).len(), 9);
    }

    #[test]
    fn only_white_phantom_rows_count_as_summons() {
        assert!(is_summon_side(2, 80));
        assert!(!is_summon_side(16, 81));
        assert!(!is_summon_side(2, -1));
        assert!(!is_summon_side(26, 80));
    }

    #[test]
    fn an_invader_inherits_no_event_ids_but_keeps_the_flags() {
        let mut entity = vec![0u8; 0x30];
        entity[0..4].copy_from_slice(&3_000_800u32.to_ne_bytes());
        entity[4..8].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0]);
        entity[0x10..0x14].copy_from_slice(&[1, 1, 0, 1]);
        entity[0x1C..0x20].copy_from_slice(&3_005_810u32.to_ne_bytes());
        entity[0x20..0x24].copy_from_slice(&u32::MAX.to_ne_bytes());
        entity[0x24..0x28].copy_from_slice(&42u32.to_ne_bytes());
        clear_entity_ids(&mut entity);
        assert_eq!(entity[0..4], u32::MAX.to_ne_bytes());
        assert_eq!(entity[4..8], [0xFF, 0xFF, 0xFF, 0]);
        assert_eq!(entity[0x10..0x14], [1, 1, 0, 1]);
        assert_eq!(entity[0x1C..0x20], u32::MAX.to_ne_bytes());
        assert_eq!(entity[0x20..0x24], u32::MAX.to_ne_bytes());
        assert_eq!(entity[0x24..0x28], 42u32.to_ne_bytes());
    }

    #[test]
    fn ended_swaps_are_named_in_the_result() {
        assert_eq!(ended_prefix(&[]), "");
        assert_eq!(ended_prefix(&["Temporary Armor", "Bombardier"]), "ended Temporary Armor and Bombardier, then ");
    }

    #[test]
    fn attunement_takes_spells_while_their_slots_fit() {
        let spells = [(1, 1), (2, 2), (3, 1), (4, 1)];
        assert_eq!(pick_attunement(&spells, 3), vec![1, 2]);
        assert_eq!(pick_attunement(&spells, 4), vec![1, 2, 3]);
        assert_eq!(pick_attunement(&[(1, 2), (2, 1)], 1), vec![2]);
        assert!(pick_attunement(&spells, 0).is_empty());
        let many: Vec<(u32, u8)> = (0..20).map(|spell| (spell, 1)).collect();
        assert_eq!(pick_attunement(&many, 30).len(), 14);
    }

    #[test]
    fn magic_entries_match_the_games_attune_routine() {
        assert_eq!(magic_entry(2_400_000, 4), 0x0000_000C_0024_9F00);
        assert_eq!(magic_entry(2_400_000, -1), 0xFFFF_FFFF_0024_9F00);
    }

    #[test]
    fn attunement_slots_follow_the_games_table() {
        assert_eq!(attunement_slots(9), 0);
        assert_eq!(attunement_slots(10), 1);
        assert_eq!(attunement_slots(24), 4);
        assert_eq!(attunement_slots(80), 9);
        assert_eq!(attunement_slots(99), 10);
    }

    #[test]
    fn dlc_pyromancies_need_their_dlc() {
        assert_eq!(pyromancies_available(false, false).len(), 26);
        assert!(!pyromancies_available(true, true).contains(&2_435_000));
        assert!(pyromancies_available(true, false).contains(&2_457_000));
        assert!(!pyromancies_available(true, false).contains(&2_660_000));
        assert_eq!(pyromancies_available(true, true).len(), 29);
    }

    #[test]
    fn freeze_clears_only_the_hit_block_bits() {
        assert_eq!(without_bits(0xFF, 0x80), 0x7F);
        assert_eq!(without_bits(0xFF, 0b0000_0011), 0xFC);
        assert_eq!(without_bits(0x05, 0b0000_0011), 0x04);
        assert_eq!(without_bits(0x04, 0b0000_0011), 0x04);
    }

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
            firestarter: Some(Firestarter {
                until: 250,
                character: "Ash".to_string(),
                weapons: vec![RemovedWeapon { slot: 1, index: 2, weapon: 3_000_000 }],
                armor: Vec::new(),
                flame: 13_400_007,
                flame_index: 40,
                flames_before: 0,
                spells: vec![2_400_000],
                spells_given: vec![2_400_000],
                attuned_before: Some(vec![0xFFFF_FFFF; 14]),
                selected_before: 3,
                requirements: vec![SpellRequirement { spell: 2_400_000, intelligence: 6, faith: 6 }],
                weapons_changed: false,
            }),
            ooga_booga: Some(OogaBooga {
                until: 260,
                character: "Ash".to_string(),
                big: true,
                weapons: vec![RemovedWeapon { slot: 1, index: 2, weapon: 3_000_000 }],
                armor: vec![ArmorSwap { slot: 12, old_index: 4, old_id: 21_000_000, owned_before: 0, piece: 0 }],
                club: 8_080_010,
                club_index: 41,
                clubs_before: 0,
                row_before: Some(ClubRow { strength: 28, dexterity: 0, intelligence: 0, faith: 0, resident_effect: -1 }),
                changed: false,
            }),
            stats: Some(ShuffledStats {
                until: 300,
                character: "Ash".to_string(),
                original: [12, 10, 11, 15, 13, 12, 9, 9, 7],
            }),
            recently_ended: vec![EndedSwap {
                kind: EndedKind::Stats(ShuffledStats {
                    until: 50,
                    character: "Ash".to_string(),
                    original: [1, 2, 3, 4, 5, 6, 7, 8, 9],
                }),
                here: Some(std::time::Instant::now()),
            }],
        };

        let text = serde_json::to_string(&saved).unwrap();
        let loaded: PendingRestores = serde_json::from_str(&text).unwrap();

        assert!(!loaded.is_empty());
        let armor = loaded.armor.unwrap();
        assert_eq!((armor.until, armor.swaps[0].piece), (100, 7));
        let bombardier = loaded.bombardier.unwrap();
        assert_eq!((bombardier.weapons[0].weapon, bombardier.quick_item), (3_000_000, 0x4000_0097));
        assert_eq!(bombardier.armor[0].piece, 79_001_000);
        let firestarter = loaded.firestarter.unwrap();
        assert_eq!((firestarter.flame, firestarter.selected_before), (13_400_007, 3));
        assert_eq!(firestarter.attuned_before.unwrap().len(), 14);
        assert_eq!(firestarter.requirements[0].intelligence, 6);
        let ooga = loaded.ooga_booga.unwrap();
        assert_eq!((ooga.big, ooga.club, ooga.armor[0].old_id), (true, 8_080_010, 21_000_000));
        assert_eq!(ooga.row_before.unwrap().strength, 28);
        assert_eq!(loaded.stats.unwrap().original[3], 15);
        // An ended swap from an earlier run is checked on the next load, not timed out.
        assert_eq!(loaded.recently_ended.len(), 1);
        assert!(loaded.recently_ended[0].here.is_none());
        assert_eq!(loaded.recently_ended[0].kind.character(), "Ash");
        assert!(PendingRestores::default().is_empty());
    }

    #[test]
    fn the_chest_in_front_is_the_one_faced_and_in_reach() {
        let chests = [[-1.8, 0.0, 1.5], [0.0, 0.0, 1.5], [1.8, 0.0, 1.5]];
        let here = [0.0, 0.0, 0.0];
        assert_eq!(super::chest_in_front(here, [0.0, 1.0], &chests), Some(1));
        // Between two chests and turned toward the right-hand one.
        let length = (0.9f32 * 0.9 + 1.0 * 1.0).sqrt();
        assert_eq!(super::chest_in_front([0.9, 0.0, 0.5], [0.9 / length, 1.0 / length], &chests), Some(2));
        // Facing away, or too far.
        assert_eq!(super::chest_in_front(here, [0.0, -1.0], &chests), None);
        assert_eq!(super::chest_in_front([0.0, 0.0, -3.0], [0.0, 1.0], &chests), None);
    }

    #[test]
    fn facing_conventions_are_read_off_a_known_direction() {
        for convention in 0..super::FACING_CONVENTIONS {
            let angle = 0.7f32;
            let forward = super::facing_direction(convention, angle);
            let picked = super::pick_facing_convention(angle, forward);
            assert_eq!(picked, convention);
            let back = super::facing_for(picked, forward[0], forward[1]);
            assert!((back - angle).abs() < 1e-4, "{convention}: {back}");
        }
    }

    #[test]
    fn a_file_saved_before_ooga_booga_still_loads() {
        let text = r#"{"stats":{"until":1,"character":"Ash","original":[1,2,3,4,5,6,7,8,9]}}"#;
        let loaded: PendingRestores = serde_json::from_str(text).unwrap();
        assert!(loaded.ooga_booga.is_none());
        assert!(loaded.stats.is_some());
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
    fn gear_snapshots_take_turns_and_keep_the_older_one() {
        let snapshot = |taken| GearSnapshot { taken, character: "Ash".to_string(), weapons: [0; 6], armor: [0; 4] };
        let mut snapshots = GearSnapshots::default();
        snapshots.store(snapshot(1));
        snapshots.store(snapshot(2));
        snapshots.store(snapshot(3));
        let taken: Vec<u64> = snapshots.slots.iter().flatten().map(|slot| slot.taken).collect();
        assert_eq!(taken, [3, 2]);
        assert_eq!(snapshots.next, 1);
    }

    #[test]
    fn a_gear_snapshot_is_due_at_start_and_every_five_minutes() {
        assert!(gear_snapshot_due(1000, None));
        assert!(!gear_snapshot_due(1000 + GEAR_SNAPSHOT_EVERY - 1, Some(1000)));
        assert!(gear_snapshot_due(1000 + GEAR_SNAPSHOT_EVERY, Some(1000)));
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
