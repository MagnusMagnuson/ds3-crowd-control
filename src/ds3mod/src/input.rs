use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::Win32::System::Memory::{VirtualProtect, PAGE_PROTECTION_FLAGS, PAGE_READWRITE};

use crate::pattern;

const INPUT_MANAGER_RVA: usize = 0x49644D0;
const INPUT_MANAGER_VTABLE_RVA: usize = 0x3A00D80;
const PAD_DEVICE_VTABLE_RVA: usize = 0x3A10438;
const VIRTUAL_INPUT_VTABLE_RVA: usize = 0x3A1E420;
const DEVICES_OFFSET: usize = 0x90;
const INPUT_DATA_OFFSET: usize = 0x140;
const BITSET_OFFSET: usize = INPUT_DATA_OFFSET + 0x30;
const XINPUT_GAMEPAD_X: u16 = 0x4000;
const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
const LEARNED_BUTTON_FILE: &str = "use-item-button.txt";
const WATCH_FRAMES: u32 = 600;

type XInputGetState = unsafe extern "system" fn(u32, *mut XInputState) -> u32;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputGamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputState {
    packet_number: u32,
    gamepad: XInputGamepad,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    FrameBegin,
    AfterPadStep,
    ChrInsPrepare,
    ChrInsPreBehavior,
}

impl Phase {
    fn label(self) -> &'static str {
        match self {
            Self::FrameBegin => "frame_begin",
            Self::AfterPadStep => "after_pad_step",
            Self::ChrInsPrepare => "chr_prepare",
            Self::ChrInsPreBehavior => "chr_pre_behavior",
        }
    }
}

#[derive(Clone)]
struct Transition {
    frame: u32,
    phase: Phase,
    before: Vec<u8>,
    after: Vec<u8>,
}

struct Trace {
    start_frame: u32,
    remaining: u32,
    bitset_address: usize,
    bitset_size: usize,
    last: Option<Vec<u8>>,
    transitions: Vec<Transition>,
    xinput: Vec<(u32, u32, u16)>,
}

static ORIGINAL_XINPUT: AtomicUsize = AtomicUsize::new(0);
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);
static PENDING_USE_ITEM: AtomicBool = AtomicBool::new(false);
static SYNTHETIC_X: AtomicBool = AtomicBool::new(false);
static SYNTHETIC_POLLS: AtomicU32 = AtomicU32::new(0);
static SYNTHETIC_CONTROLLER: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_CONTROLLER: AtomicU32 = AtomicU32::new(u32::MAX);
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(0);
/// X on the game's default controls, so Use selected item works without any setup.
static LEARNED_BUTTON: AtomicU32 = AtomicU32::new(0x4000);
static FRAME: AtomicU32 = AtomicU32::new(0);
static TRACE: Mutex<Option<Trace>> = Mutex::new(None);

pub fn prepare() {
    if let Ok(button) = load_learned_button() {
        LEARNED_BUTTON.store(button as u32, Ordering::Release);
    }
    if let Err(error) = install_xinput_hook() {
        crate::log::log(format!("XInputGetState hook unavailable: {error}"));
    }
    if let Err(error) = locate_pad_bitset() {
        crate::log::log(format!("pad bitset unavailable: {error}"));
    }
}

pub fn frame_begin() {
    let frame = FRAME.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if SYNTHETIC_X.load(Ordering::Acquire) && SYNTHETIC_POLLS.load(Ordering::Acquire) > 0 {
        SYNTHETIC_X.store(false, Ordering::Release);
        SYNTHETIC_POLLS.store(0, Ordering::Release);
    }
    if PENDING_USE_ITEM.load(Ordering::Acquire)
        && !SYNTHETIC_X.load(Ordering::Acquire)
        && matches!(crate::game::menus_open(), Ok(false))
        && activate_pending()
    {
    }
    MENU_OPEN.store(matches!(crate::game::menus_open(), Ok(true)), Ordering::Release);
    sample(Phase::FrameBegin, frame);
    let finished = {
        let mut slot = TRACE.lock().unwrap_or_else(|poison| poison.into_inner());
        let Some(trace) = slot.as_mut() else {
            return;
        };
        trace.remaining = trace.remaining.saturating_sub(1);
        if trace.remaining == 0 { slot.take() } else { None }
    };
    if let Some(trace) = finished {
        finish_trace(trace);
    }
}

pub fn sample_current(phase: Phase) {
    sample(phase, FRAME.load(Ordering::Relaxed));
}

pub fn start_trace() -> Result<String, String> {
    let (bitset_address, bitset_size) = locate_pad_bitset()?;
    let mut trace = TRACE.lock().unwrap_or_else(|poison| poison.into_inner());
    if trace.is_some() {
        return Err("a low-level input trace is already running".to_string());
    }
    *trace = Some(Trace {
        start_frame: FRAME.load(Ordering::Relaxed),
        remaining: WATCH_FRAMES,
        bitset_address,
        bitset_size,
        last: None,
        transitions: Vec::new(),
        xinput: Vec::new(),
    });
    Ok(format!(
        "Low-level input trace started at {bitset_address:#X} ({bitset_size} bytes). Press and release only the use-item button once."
    ))
}

pub fn queue_use_item(menu_open: bool) -> Result<String, String> {
    if !HOOK_INSTALLED.load(Ordering::Acquire) {
        return Err("XInputGetState is not hooked, so no input was sent".to_string());
    }
    let button = LEARNED_BUTTON.load(Ordering::Acquire) as u16;
    if button == 0 {
        return Err("the use-item controller bit is not learned yet; run Watch item button once".to_string());
    }
    if SYNTHETIC_X.load(Ordering::Acquire)
        || PENDING_USE_ITEM.swap(true, Ordering::AcqRel)
    {
        return Err("a synthetic use-item press is already pending".to_string());
    }
    if menu_open {
        return Ok("Queued use-item command; waiting for the open menu to close.".to_string());
    }
    if activate_pending() {
        Ok("Queued one controller use-item pulse; the game will decide whether the selected item can be used.".to_string())
    } else {
        Ok("Queued use-item command; waiting for the game to poll an idle controller.".to_string())
    }
}

fn activate_pending() -> bool {
    if !PENDING_USE_ITEM.load(Ordering::Acquire)
        || SYNTHETIC_X.load(Ordering::Acquire)
    {
        return false;
    }
    let controller = LAST_CONTROLLER.load(Ordering::Acquire);
    let button = LEARNED_BUTTON.load(Ordering::Acquire) as u16;
    if controller == u32::MAX
        || button == 0
        || LAST_BUTTONS.load(Ordering::Acquire) as u16 & button != 0
    {
        return false;
    }
    SYNTHETIC_CONTROLLER.store(controller, Ordering::Release);
    SYNTHETIC_POLLS.store(0, Ordering::Release);
    SYNTHETIC_X.store(true, Ordering::Release);
    PENDING_USE_ITEM.store(false, Ordering::Release);
    true
}

fn sample(phase: Phase, frame: u32) {
    let mut trace = TRACE.lock().unwrap_or_else(|poison| poison.into_inner());
    let Some(trace) = trace.as_mut() else {
        return;
    };
    let mut current = vec![0u8; trace.bitset_size];
    if copy_readable(trace.bitset_address, &mut current) != trace.bitset_size {
        return;
    }
    if let Some(before) = &trace.last {
        if before != &current && trace.transitions.len() < 512 {
            trace.transitions.push(Transition {
                frame,
                phase,
                before: before.clone(),
                after: current.clone(),
            });
        }
    }
    trace.last = Some(current);
}

fn record_xinput(index: u32, buttons: u16) {
    let frame = FRAME.load(Ordering::Relaxed);
    let mut trace = TRACE.lock().unwrap_or_else(|poison| poison.into_inner());
    let Some(trace) = trace.as_mut() else {
        return;
    };
    if trace
        .xinput
        .last()
        .is_none_or(|(_, previous_index, previous)| *previous_index != index || *previous != buttons)
        && trace.xinput.len() < 256
    {
        trace.xinput.push((frame, index, buttons));
    }
}

fn finish_trace(trace: Trace) {
    let mut report = format!(
        "startFrame={}\nframes={WATCH_FRAMES}\nbitsetAddress={:#X}\nbitsetSize={}\ntransitions={}\n",
        trace.start_frame,
        trace.bitset_address,
        trace.bitset_size,
        trace.transitions.len()
    );
    for transition in &trace.transitions {
        let changes = changed_bits(&transition.before, &transition.after);
        report.push_str(&format!(
            "frame={} phase={} bits={}\n",
            transition.frame,
            transition.phase.label(),
            changes
                .iter()
                .map(|bit| bit.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    report.push_str("\nxinput:\n");
    for (frame, index, buttons) in &trace.xinput {
        report.push_str(&format!(
            "frame={frame} controller={index} buttons={buttons:#06X} x={}\n",
            buttons & XINPUT_GAMEPAD_X != 0
        ));
    }
    match infer_button(&trace.xinput) {
        Some(button) => {
            LEARNED_BUTTON.store(button as u32, Ordering::Release);
            match save_learned_button(button) {
                Ok(()) => report.push_str(&format!(
                    "\nlearnedUseItemButton={button:#06X}\nstandardX={}\n",
                    button == XINPUT_GAMEPAD_X
                )),
                Err(error) => report.push_str(&format!(
                    "\nlearnedUseItemButton={button:#06X}\npersistError={error}\n"
                )),
            }
        }
        None => report.push_str(
            "\nlearnedUseItemButton=none (expected exactly one button to rise and fall)\n",
        ),
    }
    match write_trace_report(&report) {
        Ok(path) => crate::log::log(format!(
            "low-level input trace wrote {} transitions={} xinput={}",
            path.display(),
            trace.transitions.len(),
            trace.xinput.len()
        )),
        Err(error) => crate::log::log(format!("low-level input trace write failed: {error}")),
    }
}

fn write_trace_report(report: &str) -> Result<std::path::PathBuf, String> {
    let dir = crate::log::app_dir()?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("input-trace-{stamp}.txt"));
    std::fs::write(&path, report).map_err(|error| error.to_string())?;
    Ok(path)
}

fn infer_button(samples: &[(u32, u32, u16)]) -> Option<u16> {
    let mut rose = 0u16;
    let mut fell = 0u16;
    for pair in samples.windows(2) {
        let (_, left_index, left) = pair[0];
        let (_, right_index, right) = pair[1];
        if left_index != right_index {
            continue;
        }
        rose |= !left & right;
        fell |= left & !right;
    }
    let candidate = rose & fell;
    (candidate.count_ones() == 1).then_some(candidate)
}

fn learned_button_path() -> Result<std::path::PathBuf, String> {
    Ok(crate::log::app_dir()?.join(LEARNED_BUTTON_FILE))
}

fn save_learned_button(button: u16) -> Result<(), String> {
    let path = learned_button_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(path, format!("{button:04X}\n")).map_err(|error| error.to_string())
}

fn load_learned_button() -> Result<u16, String> {
    let text = std::fs::read_to_string(learned_button_path()?).map_err(|error| error.to_string())?;
    let button = u16::from_str_radix(text.trim().trim_start_matches("0x"), 16)
        .map_err(|error| error.to_string())?;
    if button.count_ones() != 1 {
        return Err("saved use-item button is not one bit".to_string());
    }
    Ok(button)
}

fn locate_pad_bitset() -> Result<(usize, usize), String> {
    let (_, base) = pattern::module_slice()?;
    let manager = base + INPUT_MANAGER_RVA;
    expect_vtable(manager, base + INPUT_MANAGER_VTABLE_RVA, "input manager")?;
    let first = read_usize(manager + DEVICES_OFFSET)?;
    let last = read_usize(manager + DEVICES_OFFSET + 8)?;
    let end = read_usize(manager + DEVICES_OFFSET + 16)?;
    if first < 0x10_000 || last < first || end < last || (last - first) % 8 != 0 {
        return Err("input-manager device vector is invalid".to_string());
    }
    let count = (last - first) / 8;
    if count == 0 || count > 32 {
        return Err(format!("input-manager device count {count} is invalid"));
    }
    for index in 0..count {
        let device = read_usize(first + index * 8)?;
        if device < 0x10_000 || read_usize(device).ok() != Some(base + PAD_DEVICE_VTABLE_RVA) {
            continue;
        }
        let input_data = device + INPUT_DATA_OFFSET;
        expect_vtable(input_data, base + VIRTUAL_INPUT_VTABLE_RVA, "virtual input data")?;
        let bitset = device + BITSET_OFFSET;
        let size = read_usize(bitset + 8)?;
        let data = read_usize(bitset + 16)?;
        if size == 0 || size > 0x400 {
            return Err(format!("pad bitset size {size} is invalid"));
        }
        let mut probe = vec![0u8; size];
        if copy_readable(data, &mut probe) != size {
            return Err("pad bitset data is not readable".to_string());
        }
        return Ok((data, size));
    }
    Err("no validated PadDevice was found".to_string())
}

fn expect_vtable(address: usize, expected: usize, label: &str) -> Result<(), String> {
    let actual = read_usize(address)?;
    if actual != expected {
        return Err(format!("{label} vtable was {actual:#X}, expected {expected:#X}"));
    }
    Ok(())
}

fn read_usize(address: usize) -> Result<usize, String> {
    let mut bytes = [0u8; 8];
    if copy_readable(address, &mut bytes) != bytes.len() {
        return Err(format!("address {address:#X} is not readable"));
    }
    Ok(usize::from_ne_bytes(bytes))
}

fn copy_readable(address: usize, destination: &mut [u8]) -> usize {
    use windows::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};
    if destination.is_empty() || address < 0x10_000 {
        return 0;
    }
    let mut info = MEMORY_BASIC_INFORMATION::default();
    let queried = unsafe {
        VirtualQuery(
            Some(address as *const _),
            &mut info,
            std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    };
    if queried == 0 || info.State != MEM_COMMIT || info.Protect.0 & 0x100 != 0 {
        return 0;
    }
    let protection = info.Protect.0 & 0xFF;
    if !matches!(protection, 0x02 | 0x04 | 0x08 | 0x20 | 0x40 | 0x80) {
        return 0;
    }
    let region_end = (info.BaseAddress as usize).saturating_add(info.RegionSize);
    let count = destination.len().min(region_end.saturating_sub(address));
    if count > 0 {
        unsafe {
            std::ptr::copy_nonoverlapping(
                address as *const u8,
                destination.as_mut_ptr(),
                count,
            )
        };
    }
    count
}

fn changed_bits(before: &[u8], after: &[u8]) -> Vec<usize> {
    let mut changed = Vec::new();
    for (byte_index, (left, right)) in before.iter().zip(after).enumerate() {
        let xor = left ^ right;
        for bit in 0..8 {
            if xor & (1 << bit) != 0 {
                changed.push(byte_index * 8 + bit);
            }
        }
    }
    changed
}

fn install_xinput_hook() -> Result<(), String> {
    if HOOK_INSTALLED.load(Ordering::Acquire) {
        return Ok(());
    }
    let slot = locate_import_slot("XINPUT1_3.dll", 2)?;
    let original = unsafe { slot.read_unaligned() };
    if original < 0x10_000 {
        return Err("XInputGetState IAT entry is null".to_string());
    }
    ORIGINAL_XINPUT.store(original, Ordering::Release);
    let mut old = PAGE_PROTECTION_FLAGS::default();
    unsafe {
        VirtualProtect(
            slot.cast(),
            std::mem::size_of::<usize>(),
            PAGE_READWRITE,
            &mut old,
        )
        .map_err(|error| error.to_string())?;
        slot.write_unaligned(hooked_xinput_get_state as *const () as usize);
        let mut ignored = PAGE_PROTECTION_FLAGS::default();
        VirtualProtect(slot.cast(), std::mem::size_of::<usize>(), old, &mut ignored)
            .map_err(|error| error.to_string())?;
    }
    HOOK_INSTALLED.store(true, Ordering::Release);
    Ok(())
}

/// Finds an ordinal import without constructing aligned `u64` slices. DS3's
/// XInput IAT starts at an address that is 4-byte, but not 8-byte, aligned.
fn locate_import_slot(dll_name: &str, ordinal: u16) -> Result<*mut usize, String> {
    let (image, base) = pattern::module_slice()?;
    let e_lfanew = read_image_u32(image, 0x3C)? as usize;
    let optional = e_lfanew
        .checked_add(24)
        .ok_or_else(|| "optional-header offset overflowed".to_string())?;
    if read_image_u16(image, optional)? != 0x20B {
        return Err("the executable is not PE32+".to_string());
    }
    let import_rva = read_image_u32(image, optional + 0x78)? as usize;
    if import_rva == 0 {
        return Err("the executable has no import directory".to_string());
    }
    for descriptor_index in 0..512usize {
        let descriptor = import_rva
            .checked_add(descriptor_index * 20)
            .ok_or_else(|| "import descriptor overflowed".to_string())?;
        let original_thunk = read_image_u32(image, descriptor)? as usize;
        let name_rva = read_image_u32(image, descriptor + 12)? as usize;
        let first_thunk = read_image_u32(image, descriptor + 16)? as usize;
        if original_thunk == 0 && name_rva == 0 && first_thunk == 0 {
            break;
        }
        if read_image_c_string(image, name_rva)?
            .eq_ignore_ascii_case(dll_name)
        {
            if original_thunk == 0 || first_thunk == 0 {
                return Err(format!("{dll_name} import thunks are missing"));
            }
            for index in 0..256usize {
                let thunk = read_image_u64(image, original_thunk + index * 8)?;
                if thunk == 0 {
                    break;
                }
                if thunk & (1u64 << 63) != 0 && thunk as u16 == ordinal {
                    let slot_rva = first_thunk
                        .checked_add(index * 8)
                        .ok_or_else(|| "IAT slot overflowed".to_string())?;
                    if slot_rva + 8 > image.len() {
                        return Err("IAT slot is outside the image".to_string());
                    }
                    return Ok((base + slot_rva) as *mut usize);
                }
            }
            return Err(format!("{dll_name} ordinal {ordinal} was not imported"));
        }
    }
    Err(format!("{dll_name} import descriptor was not found"))
}

fn read_image_u16(image: &[u8], offset: usize) -> Result<u16, String> {
    let bytes = image
        .get(offset..offset + 2)
        .ok_or_else(|| format!("image offset {offset:#X} is out of bounds"))?;
    Ok(u16::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_image_u32(image: &[u8], offset: usize) -> Result<u32, String> {
    let bytes = image
        .get(offset..offset + 4)
        .ok_or_else(|| format!("image offset {offset:#X} is out of bounds"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_image_u64(image: &[u8], offset: usize) -> Result<u64, String> {
    let bytes = image
        .get(offset..offset + 8)
        .ok_or_else(|| format!("image offset {offset:#X} is out of bounds"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_image_c_string(image: &[u8], offset: usize) -> Result<&str, String> {
    let tail = image
        .get(offset..)
        .ok_or_else(|| format!("string RVA {offset:#X} is out of bounds"))?;
    let length = tail
        .iter()
        .take(128)
        .position(|byte| *byte == 0)
        .ok_or_else(|| "import DLL name is not terminated".to_string())?;
    std::str::from_utf8(&tail[..length]).map_err(|error| error.to_string())
}

static INVERT_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static INVERT_CAMERA: AtomicBool = AtomicBool::new(false);
static BLOCK_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
static PACIFIST_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
/// Updated once a frame, so the controller hook never has to read the menu state itself.
static MENU_OPEN: AtomicBool = AtomicBool::new(false);

fn clock_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Negates a stick axis. The lowest value has no positive twin, so it maps to the highest.
fn invert_axis(value: i16) -> i16 {
    value.checked_neg().unwrap_or(i16::MAX)
}

fn invert_sticks(gamepad: &mut XInputGamepad, camera: bool) {
    gamepad.thumb_lx = invert_axis(gamepad.thumb_lx);
    gamepad.thumb_ly = invert_axis(gamepad.thumb_ly);
    if camera {
        gamepad.thumb_rx = invert_axis(gamepad.thumb_rx);
        gamepad.thumb_ry = invert_axis(gamepad.thumb_ry);
    }
}

/// Leaves the right stick alone, so the player can still look around.
fn blank_player_input(gamepad: &mut XInputGamepad) {
    gamepad.buttons = 0;
    gamepad.left_trigger = 0;
    gamepad.right_trigger = 0;
    gamepad.thumb_lx = 0;
    gamepad.thumb_ly = 0;
}

/// D-pad down is how the player moves through the quick slots on a controller.
pub fn switching_item() -> bool {
    LAST_BUTTONS.load(Ordering::Acquire) as u16 & XINPUT_GAMEPAD_DPAD_DOWN != 0
}

pub fn block_input(seconds: i32) {
    BLOCK_UNTIL_MS.fetch_max(clock_ms() + seconds.max(0) as u64 * 1000, Ordering::AcqRel);
}

pub fn stop_blocking_input() {
    BLOCK_UNTIL_MS.store(0, Ordering::Release);
}

/// Takes away attacking, blocking and using items. The bumpers and triggers do all the
/// attacking and blocking on a controller, and the use-item button uses the quick item.
fn remove_fighting(gamepad: &mut XInputGamepad, use_item_button: u16) {
    gamepad.buttons &= !(XINPUT_GAMEPAD_LEFT_SHOULDER | XINPUT_GAMEPAD_RIGHT_SHOULDER | use_item_button);
    gamepad.left_trigger = 0;
    gamepad.right_trigger = 0;
}

pub fn pacifist(seconds: i32) -> Result<String, String> {
    if ORIGINAL_XINPUT.load(Ordering::Acquire) == 0 {
        return Err("XInputGetState is not hooked, so attacks cannot be blocked".to_string());
    }
    PACIFIST_UNTIL_MS.fetch_max(clock_ms() + seconds.max(0) as u64 * 1000, Ordering::AcqRel);
    Ok(format!("no attacking, blocking or items for {seconds} seconds"))
}

pub fn invert_controls(seconds: i32, camera: bool) -> Result<String, String> {
    if ORIGINAL_XINPUT.load(Ordering::Acquire) == 0 {
        return Err("XInputGetState is not hooked, so the controls cannot be inverted".to_string());
    }
    let until = clock_ms() + seconds.max(0) as u64 * 1000;
    INVERT_CAMERA.store(camera, Ordering::Release);
    INVERT_UNTIL_MS.fetch_max(until, Ordering::AcqRel);
    Ok(format!("controls inverted for {seconds} seconds"))
}

unsafe extern "system" fn hooked_xinput_get_state(index: u32, state: *mut XInputState) -> u32 {
    let original = ORIGINAL_XINPUT.load(Ordering::Acquire);
    if original == 0 {
        return 1167; // ERROR_DEVICE_NOT_CONNECTED
    }
    let call: XInputGetState = unsafe { std::mem::transmute(original) };
    let result = unsafe { call(index, state) };
    if result != 0 || state.is_null() {
        return result;
    }
    let state = unsafe { &mut *state };
    let physical = state.gamepad.buttons;
    LAST_CONTROLLER.store(index, Ordering::Release);
    LAST_BUTTONS.store(physical as u32, Ordering::Release);
    record_xinput(index, physical);
    // Menus use the bumpers to switch tabs, so Pacifist leaves them alone there.
    // This runs before the Use selected item press is added, so that press still gets through.
    if clock_ms() < PACIFIST_UNTIL_MS.load(Ordering::Acquire) && !MENU_OPEN.load(Ordering::Acquire) {
        remove_fighting(&mut state.gamepad, LEARNED_BUTTON.load(Ordering::Acquire) as u16);
    }
    if SYNTHETIC_X.load(Ordering::Acquire)
        && index == SYNTHETIC_CONTROLLER.load(Ordering::Acquire)
    {
        state.gamepad.buttons = inject_button(
            physical,
            LEARNED_BUTTON.load(Ordering::Acquire) as u16,
        );
        state.packet_number = state.packet_number.wrapping_add(1);
        SYNTHETIC_POLLS.fetch_add(1, Ordering::AcqRel);
    }
    if clock_ms() < INVERT_UNTIL_MS.load(Ordering::Acquire) {
        invert_sticks(&mut state.gamepad, INVERT_CAMERA.load(Ordering::Acquire));
    }
    if clock_ms() < BLOCK_UNTIL_MS.load(Ordering::Acquire) {
        blank_player_input(&mut state.gamepad);
    }
    result
}

fn inject_button(buttons: u16, button: u16) -> u16 {
    buttons | button
}

#[cfg(test)]
mod tests {
    use super::{
        blank_player_input, changed_bits, infer_button, inject_button, invert_axis, invert_sticks,
        remove_fighting, XInputGamepad,
        XINPUT_GAMEPAD_X,
    };

    #[test]
    fn inverting_flips_the_chosen_sticks() {
        assert_eq!(invert_axis(1000), -1000);
        assert_eq!(invert_axis(-1000), 1000);
        assert_eq!(invert_axis(0), 0);
        assert_eq!(invert_axis(i16::MIN), i16::MAX);

        let mut pad = XInputGamepad {
            thumb_lx: 100,
            thumb_ly: -200,
            thumb_rx: 300,
            thumb_ry: -400,
            ..Default::default()
        };
        invert_sticks(&mut pad, false);
        assert_eq!((pad.thumb_lx, pad.thumb_ly), (-100, 200));
        assert_eq!((pad.thumb_rx, pad.thumb_ry), (300, -400));
        invert_sticks(&mut pad, true);
        assert_eq!((pad.thumb_lx, pad.thumb_rx, pad.thumb_ry), (100, -300, 400));
    }

    #[test]
    fn blocking_input_leaves_only_the_camera() {
        let mut pad = XInputGamepad {
            buttons: XINPUT_GAMEPAD_X,
            left_trigger: 200,
            right_trigger: 200,
            thumb_lx: 100,
            thumb_ly: -200,
            thumb_rx: 300,
            thumb_ry: -400,
        };
        blank_player_input(&mut pad);
        assert_eq!((pad.buttons, pad.left_trigger, pad.right_trigger), (0, 0, 0));
        assert_eq!((pad.thumb_lx, pad.thumb_ly), (0, 0));
        assert_eq!((pad.thumb_rx, pad.thumb_ry), (300, -400));
    }

    #[test]
    fn pacifist_removes_attacks_blocks_and_items_only() {
        let a_button = 0x1000;
        let mut pad = XInputGamepad {
            buttons: 0x0100 | 0x0200 | XINPUT_GAMEPAD_X | a_button,
            left_trigger: 200,
            right_trigger: 200,
            thumb_lx: 100,
            thumb_rx: 300,
            ..Default::default()
        };
        remove_fighting(&mut pad, XINPUT_GAMEPAD_X);
        assert_eq!((pad.buttons, pad.left_trigger, pad.right_trigger), (a_button, 0, 0));
        assert_eq!((pad.thumb_lx, pad.thumb_rx), (100, 300));
        assert_eq!(inject_button(pad.buttons, XINPUT_GAMEPAD_X) & XINPUT_GAMEPAD_X, XINPUT_GAMEPAD_X);
    }

    #[test]
    fn xinput_pulse_preserves_other_buttons() {
        assert_eq!(inject_button(0x1001, XINPUT_GAMEPAD_X), 0x1001 | XINPUT_GAMEPAD_X);
    }

    #[test]
    fn changed_bit_indices_are_stable() {
        assert_eq!(changed_bits(&[0, 0], &[1, 0x80]), vec![0, 15]);
    }

    #[test]
    fn isolated_press_learns_one_button() {
        let samples = [(1, 0, 0), (2, 0, XINPUT_GAMEPAD_X), (3, 0, 0)];
        assert_eq!(infer_button(&samples), Some(XINPUT_GAMEPAD_X));
        let ambiguous = [(1, 0, 0), (2, 0, 0x5000), (3, 0, 0)];
        assert_eq!(infer_button(&ambiguous), None);
    }
}
