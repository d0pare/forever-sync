//! SavedVariables sanity check (DESIGN.md §7.5): a `.lua` file that doesn't
//! parse is never committed or applied. WoW discards SavedVariables it can't
//! load, so syncing a half-written file would wipe that addon's settings.

/// `Ok(())` if `bytes` is syntactically valid Lua. Files that aren't UTF-8
/// can't be checked by the parser and are accepted as-is.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    let Ok(text) = std::str::from_utf8(bytes) else { return Ok(()) };
    // Deeply nested addon tables can exhaust the default stack in debug
    // builds, so parse on a thread with a generous stack.
    let text = text.to_owned();
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || match full_moon::parse(&text) {
            Ok(_) => Ok(()),
            Err(errors) => Err(errors
                .first()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "parse error".to_owned())),
        })
        .map_err(|e| e.to_string())?
        .join()
        .unwrap_or_else(|_| Err("Lua parser crashed".to_owned()))
}

#[cfg(test)]
mod tests {
    use super::check;

    #[test]
    fn accepts_saved_variables() {
        let sv = "\nPlaterDB = {\n[\"profiles\"] = {\n[\"Default\"] = {\n[\"x\"] = 1.5,\n[\"s\"] = \"a\\\"b\",\n},\n},\n}\nOther = nil\n";
        assert_eq!(check(sv.as_bytes()), Ok(()));
        assert_eq!(check(b""), Ok(()));
    }

    #[test]
    fn rejects_truncated_file() {
        assert!(check(b"PlaterDB = {\n[\"profiles\"] = {\n").is_err());
    }

    #[test]
    fn accepts_non_utf8() {
        assert_eq!(check(&[0xff, 0xfe, 0x00]), Ok(()));
    }
}
