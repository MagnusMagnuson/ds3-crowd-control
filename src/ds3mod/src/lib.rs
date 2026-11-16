// The effect list in game.rs is one large json! call.
#![recursion_limit = "256"]

mod armor_ids;
mod effects;
mod game;
mod input;
mod ipc;
mod log;
mod pattern;
mod perf;

use std::sync::OnceLock;
use std::time::Duration;

use crossbeam_channel::Sender;
use darksouls3::sprj::{SprjTaskGroupIndex, SprjTaskImp};
use fromsoftware_shared::SharedTaskImpExt;
use windows::Win32::Foundation::GetLastError;
use windows::Win32::System::Threading::{CreateMutexW, CreateThread, Sleep, THREAD_CREATION_FLAGS};
use windows::core::w;

use crate::ipc::Command;

static OUTBOUND: OnceLock<Sender<String>> = OnceLock::new();
static COMMANDS: OnceLock<crossbeam_channel::Receiver<Command>> = OnceLock::new();

#[no_mangle]
pub extern "system" fn DllMain(_module: *mut std::ffi::c_void, reason: u32, _: *mut std::ffi::c_void) -> i32 {
    if reason == 1 {
        unsafe {
            let mutex = match CreateMutexW(None, true, w!("Local\\Ds3CrowdControlMod")) {
                Ok(handle) => handle,
                Err(_) => return 1,
            };
            if GetLastError().0 == 183 {
                let _ = windows::Win32::Foundation::CloseHandle(mutex);
                return 1;
            }
            let _ = CreateThread(
                None,
                0,
                Some(worker_thread),
                None,
                THREAD_CREATION_FLAGS(0),
                None,
            );
            let _ = mutex;
        }
    }
    1
}

unsafe extern "system" fn worker_thread(_: *mut std::ffi::c_void) -> u32 {
    Sleep(300);
    log::init();
    if let Err(error) = std::panic::catch_unwind(run) {
        log::log(format!("worker panicked: {error:?}"));
    }
    0
}

fn run() {
    perf::name_thread("mod: start-up, then idle");
    game::prepare();
    game::load_pending_restores();
    input::prepare();
    if !game::snapshot().exe_match && !exe_name_only() {
        log::log("refusing to run outside DarkSoulsIII.exe");
    }
    let io = match ipc::connect_and_serve() {
        Ok(io) => io,
        Err(error) => {
            log::log(error);
            return;
        }
    };
    let _ = OUTBOUND.set(io.outbound.clone());
    let _ = COMMANDS.set(io.commands);
    let outbound = io.outbound;

    if !patterns_ok() {
        log::log("executable patterns did not resolve; effects will not run");
        let _ = outbound.send(ipc::hello_line());
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let _ = outbound.send(ipc::state_line());
        }
    }

    match SprjTaskImp::wait_for_instance(Duration::from_secs(120)) {
        Ok(tasks) => {
            use perf::time;
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    on_frame();
                },
                SprjTaskGroupIndex::FrameBegin,
            );
            std::mem::forget(handle);
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    time(&perf::INPUT, || input::sample_current(input::Phase::AfterPadStep));
                },
                SprjTaskGroupIndex::ObjResUpdate,
            );
            std::mem::forget(handle);
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    time(&perf::INPUT, || input::sample_current(input::Phase::ChrInsPrepare));
                },
                SprjTaskGroupIndex::ChrIns_Prepare,
            );
            std::mem::forget(handle);
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    time(&perf::INPUT, || input::sample_current(input::Phase::ChrInsPreBehavior));
                    time(&perf::STAMINA, game::poll_stamina_lock);
                    time(&perf::SPEED, game::poll_speed_changes);
                    time(&perf::FREEZE, game::poll_freeze);
                },
                SprjTaskGroupIndex::ChrIns_PreBehavior,
            );
            std::mem::forget(handle);
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    time(&perf::SPEED, game::poll_speed_changes);
                    time(&perf::FREEZE, game::poll_freeze);
                },
                SprjTaskGroupIndex::ChrIns_BehaviorSafe,
            );
            std::mem::forget(handle);
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    time(&perf::SPEED, game::poll_speed_changes);
                    time(&perf::FREEZE, game::poll_freeze);
                },
                SprjTaskGroupIndex::ChrIns_PrePhysicsSafe,
            );
            std::mem::forget(handle);
            let handle = tasks.run_recurring(
                |_data: &usize| {
                    time(&perf::OVERSATURATION, game::poll_oversaturation);
                },
                SprjTaskGroupIndex::DrawBegin,
            );
            std::mem::forget(handle);
        }
        Err(error) => {
            log::log(format!("game task manager was not ready: {error:?}"));
            let _ = outbound.send(ipc::hello_line());
        }
    }

    // Keep the worker alive. The frame hook and the socket threads do the work.
    loop {
        std::thread::sleep(Duration::from_secs(10));
    }
}

fn on_frame() {
    use perf::time;
    perf::frame_begin();
    time(&perf::INPUT, input::frame_begin);
    time(&perf::FAKE_DEATH, game::poll_fake_death);
    time(&perf::BLIND, game::poll_blind);
    time(&perf::ENEMIES, game::poll_enemy_queue);
    game::poll_spawned_npcs();
    time(&perf::AI_TARGET_LIST, game::poll_ai_target_list);
    time(&perf::TEMPORARY_ARMOR, game::poll_temporary_armor);
    time(&perf::BOMBARDIER, game::poll_bombardier);
    time(&perf::FIRESTARTER, game::poll_firestarter);
    time(&perf::OOGA_BOOGA, game::poll_ooga_booga);
    game::poll_recently_ended();
    game::poll_mimic_test();
    game::poll_animation_scan();
    time(&perf::MIMIC_CHESTS, game::poll_mimic_chests);
    game::poll_vanishing();
    time(&perf::GEAR, game::poll_gear_snapshots);
    time(&perf::STATS, game::poll_shuffled_stats);
    time(&perf::ONE_HIT, game::poll_one_hit_run);
    time(&perf::INVINCIBLE, game::poll_invincibility);
    time(&perf::SPEED, game::poll_speed_changes);
    time(&perf::FREEZE, game::poll_freeze);
    time(&perf::STAMINA, game::poll_stamina_lock);
    time(&perf::BUTTON_WATCH, game::poll_button_watch);
    let Some(outbound) = OUTBOUND.get() else {
        return;
    };
    time(&perf::MANAGER, || {
        static HELLO_CONNECTION: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        static FRAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let connection = ipc::connection_count();
        if HELLO_CONNECTION.swap(connection, std::sync::atomic::Ordering::Relaxed) != connection {
            let _ = outbound.send(ipc::hello_line());
        }
        let frame = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if frame % 30 == 0 {
            let _ = outbound.send(ipc::state_line());
        }
    });
    let Some(commands) = COMMANDS.get() else {
        return;
    };
    for _ in 0..8 {
        let command = match commands.try_recv() {
            Ok(command) => command,
            Err(_) => break,
        };
        let Command::Trigger {
            request_id,
            handler,
            args,
        } = command;
        let outcome = perf::time_start(&handler, || effects::dispatch(&handler, &args));
        let _ = outbound.send(ipc::result_line(&request_id, &outcome));
    }
}

fn patterns_ok() -> bool {
    std::panic::catch_unwind(|| {
        let bundle = darksouls3::rva::get();
        let program = fromsoftware_shared::Program::current();
        use pelite::pe64::Pe;
        program.rva_to_va(bundle.game_data_man_ptr).is_ok()
            && program.rva_to_va(bundle.register_task).is_ok()
    })
    .unwrap_or(false)
}

fn exe_name_only() -> bool {
    use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
    let mut buffer = [0u16; 520];
    let length = unsafe { GetModuleFileNameW(None, &mut buffer) } as usize;
    if length == 0 {
        return false;
    }
    String::from_utf16_lossy(&buffer[..length])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .eq_ignore_ascii_case("DarkSoulsIII.exe")
}

