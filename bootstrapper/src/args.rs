//! Command line: no arguments = the window; `/S` = silent (IT); `/log <path>` = also write a log.
//! Unknown arguments are refused (exit 87, ERROR_INVALID_PARAMETER) rather than ignored.

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Args {
    pub silent: bool,
    pub log: Option<String>,
    pub help: bool,
}

pub const ERROR_INVALID_PARAMETER: u32 = 87;

pub fn parse<I: IntoIterator<Item = String>>(it: I) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = it.into_iter();
    while let Some(arg) = it.next() {
        match arg.to_ascii_lowercase().as_str() {
            "/s" | "-s" | "/silent" | "--silent" => a.silent = true,
            "/log" | "-log" | "--log" => {
                let p = it.next().ok_or_else(|| "/log needs a file path".to_string())?;
                if p.is_empty() {
                    return Err("/log needs a file path".into());
                }
                a.log = Some(p);
            }
            "/?" | "-h" | "--help" | "/help" => a.help = true,
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok(a)
}

pub const USAGE: &str = "Usage: setup.exe [/S] [/log <file>]\n  /S          silent install, no window; the exit code is the msiexec exit code\n  /log <file> also write a log of every step to <file>";

#[cfg(test)]
mod tests {
    use super::*;
    fn p(v: &[&str]) -> Result<Args, String> {
        parse(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn cases() {
        assert_eq!(p(&[]).unwrap(), Args::default());
        assert!(p(&["/S"]).unwrap().silent);
        assert!(p(&["/s"]).unwrap().silent);
        let a = p(&["/S", "/log", "C:\\t\\x.log"]).unwrap();
        assert_eq!(a.log.as_deref(), Some("C:\\t\\x.log"));
        assert!(p(&["/log"]).is_err());
        assert!(p(&["/quiet"]).is_err());
        assert!(p(&["/?"]).unwrap().help);
    }
}
