use windows::Win32::System::LibraryLoader::GetModuleHandleW;

pub fn module_slice() -> Result<(&'static [u8], usize), String> {
    unsafe {
        let module = GetModuleHandleW(None).map_err(|error| error.to_string())?;
        let base = module.0 as *const u8;
        if base.is_null() {
            return Err("the process executable is not loaded".to_string());
        }
        let e_lfanew = base.add(0x3C).cast::<i32>().read_unaligned() as usize;
        let size_of_image = base.add(e_lfanew + 0x50).cast::<u32>().read_unaligned() as usize;
        if size_of_image < 0x1000 {
            return Err("the executable image size is not usable".to_string());
        }
        Ok((std::slice::from_raw_parts(base, size_of_image), base as usize))
    }
}

pub fn find_unique(pattern: &str) -> Result<usize, String> {
    let (image, base) = module_slice()?;
    let parsed = parse_pattern(pattern)?;
    let mut found = None;
    let mut count = 0usize;
    let Some(anchor) = parsed.bytes.iter().position(|byte| !byte.wildcard) else {
        return Err("pattern is only wildcards".to_string());
    };
    let needle = parsed.bytes[anchor].value;
    let mut cursor = anchor;
    while cursor < image.len() {
        let Some(relative) = image[cursor..].iter().position(|byte| *byte == needle) else {
            break;
        };
        let site = cursor + relative - anchor;
        cursor = cursor + relative + 1;
        if site + parsed.len() > image.len() {
            continue;
        }
        if parsed.matches(&image[site..]) {
            count += 1;
            if count == 1 {
                found = Some(base + site);
            }
        }
    }
    match (count, found) {
        (1, Some(address)) => Ok(address),
        (0, _) => Err(format!("pattern not found: {pattern}")),
        (count, _) => Err(format!("pattern matched {count} times: {pattern}")),
    }
}

pub fn rip_target(site: usize, displacement_offset: usize, instruction_len: usize) -> Result<usize, String> {
    let (image, base) = module_slice()?;
    let index = site.checked_sub(base).ok_or("address is outside the executable")?;
    let end = index + displacement_offset + 4;
    if end > image.len() {
        return Err("RIP displacement runs past the image".to_string());
    }
    let disp = i32::from_le_bytes(image[index + displacement_offset..end].try_into().unwrap());
    Ok(site + instruction_len + disp as isize as usize)
}

#[derive(Clone)]
struct PatternByte {
    value: u8,
    wildcard: bool,
}

struct Pattern {
    bytes: Vec<PatternByte>,
}

impl Pattern {
    fn len(&self) -> usize {
        self.bytes.len()
    }

    fn matches(&self, haystack: &[u8]) -> bool {
        self.bytes.iter().enumerate().all(|(index, byte)| {
            byte.wildcard || haystack.get(index) == Some(&byte.value)
        })
    }
}

fn parse_pattern(pattern: &str) -> Result<Pattern, String> {
    let mut bytes = Vec::new();
    for token in pattern.split_whitespace() {
        if token == "??" || token == "?" {
            bytes.push(PatternByte { value: 0, wildcard: true });
        } else {
            let value = u8::from_str_radix(token, 16)
                .map_err(|_| format!("bad pattern byte '{token}'"))?;
            bytes.push(PatternByte { value, wildcard: false });
        }
    }
    if bytes.is_empty() {
        return Err("empty pattern".to_string());
    }
    Ok(Pattern { bytes })
}

#[cfg(test)]
mod tests {
    use super::parse_pattern;

    #[test]
    fn pattern_parser_accepts_wildcards() {
        let parsed = parse_pattern("48 8B ?? 04").unwrap();
        assert!(parsed.matches(&[0x48, 0x8B, 0x05, 0x04]));
        assert!(!parsed.matches(&[0x48, 0x8B, 0x05, 0x05]));
    }
}
