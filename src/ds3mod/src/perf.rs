//! Measures how smooth the game runs and how much of that is the mod.
//!
//! - Every piece of mod code that runs inside the game is timed: the per-frame checks of
//!   each effect, starting effects, and the three hooks (only the mod's own work in them,
//!   not the game's original function they pass through to).
//! - Every frame's length is recorded together with the mod's time inside that frame, so a
//!   slow frame can be checked for whether the mod was busy in it.
//! - What the clock reads themselves cost is measured once and taken out of the figures.
//! - How busy each thread of the game was, to see whether one thread is the limit.
//!
//! It is off until "Toggle performance measuring" switches it on; while off, each timed
//! part costs one check of a switch. "Performance report" shows what was measured since
//! the last report and starts over. The frame the report runs in, and the one after, are
//! left out, so the report cannot show up as a slow frame of its own.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub struct Part {
    name: &'static str,
    /// False for a step inside another part: it is listed on its own but not added to the
    /// frame's total a second time.
    in_frame: bool,
    nanos: AtomicU64,
    calls: AtomicU64,
    longest: AtomicU64,
}

impl Part {
    const fn new(name: &'static str) -> Self {
        Self { name, in_frame: true, nanos: AtomicU64::new(0), calls: AtomicU64::new(0), longest: AtomicU64::new(0) }
    }

    const fn inside(name: &'static str) -> Self {
        Self { name, in_frame: false, nanos: AtomicU64::new(0), calls: AtomicU64::new(0), longest: AtomicU64::new(0) }
    }

    fn add(&'static self, nanos: u64) {
        self.nanos.fetch_add(nanos, Relaxed);
        self.calls.fetch_add(1, Relaxed);
        self.longest.fetch_max(nanos, Relaxed);
        if !self.in_frame {
            if nanos > FRAME_WORST_INSIDE_NANOS.fetch_max(nanos, Relaxed) {
                FRAME_WORST_INSIDE_PART.store(self as *const Part as usize, Relaxed);
            }
            return;
        }
        FRAME_MOD_NANOS.fetch_add(nanos, Relaxed);
        FRAME_MOD_CALLS.fetch_add(1, Relaxed);
        if nanos > FRAME_WORST_NANOS.fetch_max(nanos, Relaxed) {
            FRAME_WORST_PART.store(self as *const Part as usize, Relaxed);
        }
    }

    fn take(&self) -> (u64, u64, u64) {
        (self.nanos.swap(0, Relaxed), self.calls.swap(0, Relaxed), self.longest.swap(0, Relaxed))
    }
}

macro_rules! parts {
    ($($id:ident: $kind:ident $name:expr,)*) => {
        $(pub static $id: Part = Part::$kind($name);)*
        static ALL: &[&Part] = &[$(&$id),*];
    };
}

parts! {
    INPUT: new "Controller input checks (Invert controls, Pacifist, Use item)",
    XINPUT_HOOK: new "Hook: controller read (mod's part only)",
    FAKE_DEATH: new "Fake Death and Show YOU DIED",
    BLIND: new "Blind",
    ENEMIES: new "Spawn enemies, invaders and allies",
    NPC_BUILD: inside "  inside: the game building an ally or invader",
    NPC_FINISH: inside "  inside: turning it into a phantom and the rest",
    LATE_NPC_SETUP: new "Allies and invaders: setup after loading",
    PHANTOM_ROUTINE: inside "  inside: the game's phantom routine (also at spawn)",
    AI_TARGET_LIST: new "Allies and invaders: AI target list",
    AI_LIST_GROW: inside "  inside: putting an NPC back on it (moving the list if full)",
    SIMULATED_LIST: new "Allies and invaders: list of NPCs simulated here",
    SIMULATED_HOOK: new "Hook: who is simulated here (mod's part only)",
    NAME_HOOK: new "Hook: character names (mod's part only)",
    TEMPORARY_ARMOR: new "Temporary armor",
    BOMBARDIER: new "Bombardier",
    FIRESTARTER: new "Firestarter",
    OOGA_BOOGA: new "Ooga Booga",
    MIMIC_CHESTS: new "Mimic Chests",
    GEAR: new "Gear snapshots",
    STATS: new "Shuffled stats",
    ONE_HIT: new "One-hit run",
    INVINCIBLE: new "Invincible",
    SPEED: new "Speed changes",
    FREEZE: new "Freeze",
    STAMINA: new "Stamina lock",
    BUTTON_WATCH: new "Watch item button",
    OVERSATURATION: new "Oversaturation",
    MANAGER: new "Talking to the manager",
    STARTING: new "Starting effects",
    LOG: inside "  inside any of the above: handing a line to the log",
}

/// Frames longer than these count as hitches.
const HITCH_NANOS: u64 = 25_000_000;
const BAD_HITCH_NANOS: u64 = 50_000_000;
/// Longer than this is a load screen or a paused game, not a frame.
const NOT_A_FRAME_NANOS: u64 = 1_000_000_000;
/// Starting an effect that takes longer than this is written to the log by name.
const SLOW_START_NANOS: u64 = 2_000_000;
/// Frame lengths are counted in steps of 0.1 ms up to 100 ms.
const HISTOGRAM_STEP_NANOS: u64 = 100_000;
const HISTOGRAM_STEPS: usize = 1000;
/// The slow frames listed one by one in the log.
const SLOW_FRAMES_LISTED: usize = 60;

static ENABLED: AtomicBool = AtomicBool::new(false);
static EPOCH: OnceLock<Instant> = OnceLock::new();
static LAST_FRAME: AtomicU64 = AtomicU64::new(0);
static SKIP_FRAMES: AtomicU32 = AtomicU32::new(0);
static FRAME_MOD_NANOS: AtomicU64 = AtomicU64::new(0);
static FRAME_MOD_CALLS: AtomicU64 = AtomicU64::new(0);
static FRAME_WORST_NANOS: AtomicU64 = AtomicU64::new(0);
static FRAME_WORST_PART: AtomicUsize = AtomicUsize::new(0);
static FRAME_WORST_INSIDE_NANOS: AtomicU64 = AtomicU64::new(0);
static FRAME_WORST_INSIDE_PART: AtomicUsize = AtomicUsize::new(0);
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);
static FRAME_STATS: Mutex<Option<FrameStats>> = Mutex::new(None);
static THREAD_NAMES: Mutex<Vec<(u32, &'static str)>> = Mutex::new(Vec::new());
/// What one timed step adds to its own figure, and what it costs in all, in nanoseconds.
static CLOCK_COST: OnceLock<(u64, f64)> = OnceLock::new();
static CALIBRATION: Part = Part::new("calibration");

fn now() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

/// Runs `work` and adds the time it took to `part`.
pub fn time<T>(part: &'static Part, work: impl FnOnce() -> T) -> T {
    if !ENABLED.load(Relaxed) {
        return work();
    }
    let start = Instant::now();
    let result = work();
    part.add(start.elapsed().as_nanos() as u64);
    result
}

/// Adds time measured elsewhere to `part`.
pub fn add(part: &'static Part, elapsed: std::time::Duration) {
    if ENABLED.load(Relaxed) {
        part.add(elapsed.as_nanos() as u64);
    }
}

/// Starts one effect and logs it by name if starting it was slow.
pub fn time_start<T>(handler: &str, work: impl FnOnce() -> T) -> T {
    if !ENABLED.load(Relaxed) {
        return work();
    }
    let start = Instant::now();
    let result = work();
    let nanos = start.elapsed().as_nanos() as u64;
    STARTING.add(nanos);
    if nanos > SLOW_START_NANOS {
        crate::log::log(format!("performance: starting {handler} took {:.2} ms", nanos as f64 / 1e6));
    }
    result
}

/// Puts a name on the calling thread for the report.
pub fn name_thread(name: &'static str) {
    let id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    THREAD_NAMES.lock().unwrap_or_else(|poison| poison.into_inner()).push((id, name));
}

fn clock_cost() -> (u64, f64) {
    *CLOCK_COST.get_or_init(|| {
        const ROUNDS: u64 = 8192;
        // What a timed step reports when there is nothing in it.
        let mut empty = 0u64;
        for _ in 0..ROUNDS {
            let start = Instant::now();
            std::hint::black_box(());
            empty += start.elapsed().as_nanos() as u64;
        }
        // What a timed step costs in all, the counting included.
        let outer = Instant::now();
        for _ in 0..ROUNDS {
            let start = Instant::now();
            std::hint::black_box(());
            let nanos = start.elapsed().as_nanos() as u64;
            CALIBRATION.nanos.fetch_add(nanos, Relaxed);
            CALIBRATION.calls.fetch_add(1, Relaxed);
            CALIBRATION.longest.fetch_max(nanos, Relaxed);
            std::hint::black_box(FRAME_MOD_NANOS.load(Relaxed));
            std::hint::black_box(FRAME_WORST_NANOS.fetch_max(0, Relaxed));
        }
        let full = outer.elapsed().as_nanos() as f64 / ROUNDS as f64;
        (empty / ROUNDS, full)
    })
}

struct SlowFrame {
    at: f64,
    length: u64,
    mod_nanos: u64,
    worst: Option<(&'static str, u64)>,
    worst_inside: Option<(&'static str, u64)>,
}

struct FrameStats {
    since: Instant,
    cpu: cpu::Snapshot,
    frames: u64,
    frame_nanos: u64,
    longest: u64,
    histogram: Vec<u32>,
    hitches: u64,
    bad_hitches: u64,
    mod_nanos: u64,
    mod_longest_frame: u64,
    mod_frames_over_1ms: u64,
    mod_in_slow_frames: u64,
    slow: Vec<SlowFrame>,
}

impl FrameStats {
    fn new() -> Self {
        Self {
            since: Instant::now(),
            cpu: cpu::snapshot(),
            frames: 0,
            frame_nanos: 0,
            longest: 0,
            histogram: vec![0; HISTOGRAM_STEPS],
            hitches: 0,
            bad_hitches: 0,
            mod_nanos: 0,
            mod_longest_frame: 0,
            mod_frames_over_1ms: 0,
            mod_in_slow_frames: 0,
            slow: Vec::new(),
        }
    }

    fn percentile(&self, share: f64) -> f64 {
        let wanted = ((self.frames as f64) * share).ceil().max(1.0) as u64;
        let mut seen = 0u64;
        for (step, count) in self.histogram.iter().enumerate() {
            seen += u64::from(*count);
            if seen >= wanted {
                return ((step as u64 + 1) * HISTOGRAM_STEP_NANOS) as f64 / 1e6;
            }
        }
        self.longest as f64 / 1e6
    }
}

fn part_name(address: usize) -> Option<&'static str> {
    ALL.iter().find(|part| std::ptr::eq(**part, address as *const Part)).map(|part| part.name)
}

/// Switches measuring on, from a clean start, or off with a last report.
pub fn toggle() -> Result<String, String> {
    if ENABLED.load(Relaxed) {
        let last = report();
        ENABLED.store(false, Relaxed);
        *FRAME_STATS.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
        return Ok(match last {
            Ok(summary) => format!("performance measuring is off. Last report: {summary}"),
            Err(_) => "performance measuring is off".to_string(),
        });
    }
    // Whatever is left from an earlier measurement must not count.
    for part in ALL {
        part.take();
    }
    for counter in [&FRAME_MOD_NANOS, &FRAME_MOD_CALLS, &FRAME_WORST_NANOS, &FRAME_WORST_INSIDE_NANOS, &LAST_FRAME] {
        counter.store(0, Relaxed);
    }
    *FRAME_STATS.lock().unwrap_or_else(|poison| poison.into_inner()) = None;
    ENABLED.store(true, Relaxed);
    Ok("measuring performance until you trigger this again; Performance report shows what was measured so far".to_string())
}

/// Called once at the start of every game frame, on the game's main thread.
pub fn frame_begin() {
    if !ENABLED.load(Relaxed) {
        return;
    }
    let now = now();
    let last = LAST_FRAME.swap(now, Relaxed);
    let mod_nanos = FRAME_MOD_NANOS.swap(0, Relaxed);
    let mod_calls = FRAME_MOD_CALLS.swap(0, Relaxed);
    let worst_nanos = FRAME_WORST_NANOS.swap(0, Relaxed);
    let worst_part = FRAME_WORST_PART.swap(0, Relaxed);
    let worst_inside_nanos = FRAME_WORST_INSIDE_NANOS.swap(0, Relaxed);
    let worst_inside_part = FRAME_WORST_INSIDE_PART.swap(0, Relaxed);
    let mut slot = FRAME_STATS.lock().unwrap_or_else(|poison| poison.into_inner());
    let stats = match slot.as_mut() {
        Some(stats) => stats,
        None => {
            // The first frame: work out what the clock costs and look at the threads once.
            // That takes a moment, so the frame it happens in is not counted.
            MAIN_THREAD.store(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, Relaxed);
            clock_cost();
            *slot = Some(FrameStats::new());
            SKIP_FRAMES.store(1, Relaxed);
            return;
        }
    };
    if last == 0 {
        return;
    }
    if SKIP_FRAMES.load(Relaxed) > 0 {
        SKIP_FRAMES.fetch_sub(1, Relaxed);
        return;
    }
    let length = now - last;
    if length > NOT_A_FRAME_NANOS {
        return;
    }
    let (bias, _) = clock_cost();
    let mod_nanos = mod_nanos.saturating_sub(mod_calls * bias);
    stats.frames += 1;
    stats.frame_nanos += length;
    stats.longest = stats.longest.max(length);
    let step = ((length / HISTOGRAM_STEP_NANOS) as usize).min(HISTOGRAM_STEPS - 1);
    stats.histogram[step] += 1;
    stats.mod_nanos += mod_nanos;
    stats.mod_longest_frame = stats.mod_longest_frame.max(mod_nanos);
    if mod_nanos > 1_000_000 {
        stats.mod_frames_over_1ms += 1;
    }
    if length > HITCH_NANOS {
        stats.hitches += 1;
        stats.mod_in_slow_frames = stats.mod_in_slow_frames.max(mod_nanos);
        if length > BAD_HITCH_NANOS {
            stats.bad_hitches += 1;
        }
        if stats.slow.len() < SLOW_FRAMES_LISTED {
            let at = stats.since.elapsed().as_secs_f64();
            let worst = part_name(worst_part).map(|name| (name, worst_nanos.saturating_sub(bias)));
            let worst_inside = part_name(worst_inside_part)
                .map(|name| (name.trim_start(), worst_inside_nanos.saturating_sub(bias)))
                .filter(|(_, nanos)| *nanos > 100_000);
            stats.slow.push(SlowFrame { at, length, mod_nanos, worst, worst_inside });
        }
    }
}

/// What was measured since the last report, short for the manager. The full report goes
/// to the log. Starts the measurement over.
pub fn report() -> Result<String, String> {
    if !ENABLED.load(Relaxed) {
        return Err("performance measuring is off; switch it on with Toggle performance measuring first".to_string());
    }
    let (bias, full_cost) = clock_cost();
    let previous = {
        let mut slot = FRAME_STATS.lock().unwrap_or_else(|poison| poison.into_inner());
        slot.replace(FrameStats::new())
    };
    // This frame does the report's work, and the next one may still feel it.
    SKIP_FRAMES.store(2, Relaxed);
    let parts: Vec<(&str, u64, u64, u64)> = ALL
        .iter()
        .map(|part| {
            let (nanos, calls, longest) = part.take();
            (part.name, nanos.saturating_sub(calls * bias), calls, longest.saturating_sub(bias))
        })
        .collect();
    let Some(stats) = previous else {
        return Err("nothing measured yet; play for a while and try again".to_string());
    };
    if stats.frames == 0 || stats.frame_nanos == 0 {
        return Err("no game frames measured yet; play for a while and try again".to_string());
    }
    // Kept in their listed order, so each "inside" step stays under the part it is in.
    let parts: Vec<_> = parts.into_iter().filter(|part| part.2 > 0).collect();
    let frames = stats.frames as f64;
    let seconds = stats.since.elapsed().as_secs_f64();
    let average_ms = stats.frame_nanos as f64 / frames / 1e6;
    let fps = frames / (stats.frame_nanos as f64 / 1e9);
    let (p50, p95, p99, p999) = (stats.percentile(0.5), stats.percentile(0.95), stats.percentile(0.99), stats.percentile(0.999));
    let mod_average_ms = stats.mod_nanos as f64 / frames / 1e6;
    let mod_share = stats.mod_nanos as f64 / stats.frame_nanos as f64 * 100.0;
    let calls: u64 = parts.iter().map(|part| part.2).sum();
    let measuring_us = calls as f64 * full_cost / frames / 1e3;
    let usage = cpu::usage(&stats.cpu, &cpu::snapshot(), MAIN_THREAD.load(Relaxed));
    let memory = crate::game::memory_check_cost();

    let ms = |nanos: u64| nanos as f64 / 1e6;
    let mut lines = vec![
        format!("performance report over {seconds:.0} s, {} frames", stats.frames),
        format!(
            "performance: frames: {fps:.1} fps on average ({average_ms:.2} ms); half under {p50:.1} ms, 95% under {p95:.1} ms, \
             99% under {p99:.1} ms, 99.9% under {p999:.1} ms; slowest {:.1} ms; {} over 25 ms, {} over 50 ms",
            ms(stats.longest),
            stats.hitches,
            stats.bad_hitches,
        ),
        format!(
            "performance: mod: {mod_average_ms:.4} ms per frame on average ({mod_share:.3}% of the time), at most {:.3} ms in one frame, \
             {} frames where it used over 1 ms; in the slow frames (over 25 ms) at most {:.3} ms",
            ms(stats.mod_longest_frame),
            stats.mod_frames_over_1ms,
            ms(stats.mod_in_slow_frames),
        ),
        format!(
            "performance: measuring: a timed step costs about {full_cost:.0} ns ({bias} ns of it lands inside the step and is taken out \
             of every figure here), about {measuring_us:.1} µs per frame in all"
        ),
    ];
    if let Some((query, read)) = memory {
        lines.push(format!(
            "performance: memory checks on the player: asking Windows about the region {query:.1} µs, a guarded read {read:.1} µs              (the mod uses the guarded read)"
        ));
    }
    match &usage {
        Ok(usage) => {
            lines.push(format!(
                "performance: CPU: the game used {:.1}% of the PC ({} logical cores), {:.0}% of one core's worth; \
                 its main thread, which also runs the mod's frame work, {:.0}% of one core",
                usage.process_share_of_pc, usage.cores, usage.process_cores * 100.0, usage.main_thread * 100.0
            ));
            for thread in &usage.threads {
                lines.push(format!("performance:   thread {:>6} {:<44} {:>5.1}% of one core", thread.id, thread.label, thread.share * 100.0));
            }
        }
        Err(error) => lines.push(format!("performance: CPU use not available: {error}")),
    }
    lines.push("performance: mod parts (per frame is averaged over all frames; an \"inside\" line is part of the line above it, or of any for the log):".to_string());
    for (name, nanos, calls, longest) in &parts {
        lines.push(format!(
            "performance:   {name}: {:.2} µs per frame, {:.2} µs per call, longest {:.3} ms, {calls} calls",
            *nanos as f64 / frames / 1e3,
            *nanos as f64 / *calls as f64 / 1e3,
            ms(*longest),
        ));
    }
    if !stats.slow.is_empty() {
        lines.push(format!(
            "performance: slow frames (over 25 ms){}:",
            if stats.hitches as usize > stats.slow.len() { format!(", the first {}", stats.slow.len()) } else { String::new() }
        ));
        for frame in &stats.slow {
            let mut worst = frame
                .worst
                .map(|(name, nanos)| format!(", most of it {name} {:.3} ms", ms(nanos)))
                .unwrap_or_default();
            if let Some((name, nanos)) = frame.worst_inside {
                worst.push_str(&format!(" ({name}: {:.3} ms)", ms(nanos)));
            }
            lines.push(format!(
                "performance:   at {:.1} s: {:.1} ms, the mod {:.3} ms of it{worst}",
                frame.at,
                ms(frame.length),
                ms(frame.mod_nanos)
            ));
        }
    }
    for line in &lines {
        crate::log::log(line);
    }

    let cpu_text = match &usage {
        Ok(usage) => format!(
            " CPU: main game thread {:.0}% of one core, whole game {:.1}% of the PC.",
            usage.main_thread * 100.0,
            usage.process_share_of_pc
        ),
        Err(_) => String::new(),
    };
    Ok(format!(
        "{seconds:.0} s at {fps:.1} fps. Frames: half under {p50:.1} ms, 99% under {p99:.1} ms, slowest {:.1} ms, {} over 25 ms. \
         Mod: {mod_average_ms:.4} ms per frame, at most {:.3} ms in one frame, at most {:.3} ms in a slow frame.{cpu_text} Full report in ds3mod.log",
        ms(stats.longest),
        stats.hitches,
        ms(stats.mod_longest_frame),
        ms(stats.mod_in_slow_frames),
    ))
}

mod cpu {
    use super::{HashMap, Instant, THREAD_NAMES};
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, GetThreadTimes, OpenThread,
        THREAD_QUERY_LIMITED_INFORMATION,
    };

    pub struct Snapshot {
        at: Instant,
        process: u64,
        threads: HashMap<u32, u64>,
    }

    pub struct Thread {
        pub id: u32,
        pub label: String,
        pub share: f64,
    }

    pub struct Usage {
        pub cores: usize,
        pub process_cores: f64,
        pub process_share_of_pc: f64,
        pub main_thread: f64,
        pub threads: Vec<Thread>,
    }

    fn ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    /// CPU time used so far by the whole game and by each of its threads, in 100 ns ticks.
    pub fn snapshot() -> Snapshot {
        let mut threads = HashMap::new();
        let mut process = 0;
        unsafe {
            let (mut created, mut exited, mut kernel, mut user) =
                (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user).is_ok() {
                process = ticks(kernel) + ticks(user);
            }
            let own = GetCurrentProcessId();
            if let Ok(list) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
                let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
                let mut more = Thread32First(list, &mut entry).is_ok();
                while more {
                    if entry.th32OwnerProcessID == own {
                        if let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, entry.th32ThreadID) {
                            if GetThreadTimes(thread, &mut created, &mut exited, &mut kernel, &mut user).is_ok() {
                                threads.insert(entry.th32ThreadID, ticks(kernel) + ticks(user));
                            }
                            let _ = CloseHandle(thread);
                        }
                    }
                    more = Thread32Next(list, &mut entry).is_ok();
                }
                let _ = CloseHandle(list);
            }
        }
        Snapshot { at: Instant::now(), process, threads }
    }

    /// How busy the game and its threads were between two snapshots, as shares of one core.
    pub fn usage(before: &Snapshot, after: &Snapshot, main_thread: u32) -> Result<Usage, String> {
        let wall = after.at.duration_since(before.at).as_nanos() as f64 / 100.0;
        if wall <= 0.0 || after.threads.is_empty() {
            return Err("no thread times".to_string());
        }
        let cores = std::thread::available_parallelism().map(|cores| cores.get()).unwrap_or(1);
        let names = THREAD_NAMES.lock().unwrap_or_else(|poison| poison.into_inner()).clone();
        let mut threads: Vec<Thread> = after
            .threads
            .iter()
            .map(|(id, used)| {
                let share = used.saturating_sub(before.threads.get(id).copied().unwrap_or(0)) as f64 / wall;
                let label = if *id == main_thread {
                    "main game thread (also runs the mod's frame work)".to_string()
                } else if let Some((_, name)) = names.iter().find(|(named, _)| named == id) {
                    name.to_string()
                } else if !before.threads.contains_key(id) {
                    "game thread, started during the measurement".to_string()
                } else {
                    "game thread".to_string()
                };
                Thread { id: *id, label, share }
            })
            .collect();
        threads.sort_by(|a, b| b.share.total_cmp(&a.share));
        let main = threads.iter().find(|thread| thread.id == main_thread).map(|thread| thread.share).unwrap_or(0.0);
        // The busiest threads, and the mod's own ones wherever they are.
        let mut shown: Vec<Thread> = Vec::new();
        for (index, thread) in threads.into_iter().enumerate() {
            if index < 10 || thread.label.starts_with("mod:") || thread.id == main_thread {
                shown.push(thread);
            }
        }
        let process_cores = after.process.saturating_sub(before.process) as f64 / wall;
        Ok(Usage {
            cores,
            process_cores,
            process_share_of_pc: process_cores / cores as f64 * 100.0,
            main_thread: main,
            threads: shown,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameStats, HISTOGRAM_STEP_NANOS};

    #[test]
    fn percentiles_come_from_the_frame_counts() {
        let mut stats = FrameStats::new();
        // 90 frames of 16.6 ms, 9 of 20 ms and one of 45 ms.
        for (length, count) in [(16_600_000u64, 90u32), (20_000_000, 9), (45_000_000, 1)] {
            stats.histogram[(length / HISTOGRAM_STEP_NANOS) as usize] += count;
            stats.frames += u64::from(count);
            stats.longest = stats.longest.max(length);
        }
        assert!((stats.percentile(0.5) - 16.7).abs() < 0.01);
        assert!((stats.percentile(0.95) - 20.1).abs() < 0.01);
        assert!((stats.percentile(1.0) - 45.1).abs() < 0.01);
    }
}
