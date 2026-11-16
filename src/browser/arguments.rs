#[derive(Debug, PartialEq, Eq)]
pub enum Invocation {
    Browser,
    Helper,
}

/// Normal app arguments pass through unchanged. Helper classification is
/// not authentication of parentage. The sole exception recognizes Chromium 156's
/// generated custom-service zygote protocol; it never authorizes renderer bypass.
pub fn parse_arguments(args: &[String]) -> Result<Invocation, String> {
    let mut helper_type = None;
    for argument in args {
        let key = argument.split('=').next().unwrap_or(argument);
        if key.starts_with('-') && key.trim_start_matches('-').eq_ignore_ascii_case("type") {
            let Some(kind) = argument.strip_prefix("--type=") else {
                return Err("helper type must use canonical --type=VALUE".into());
            };
            if helper_type.replace(kind).is_some() {
                return Err("duplicate or conflicting helper types".into());
            }
            if ![
                "renderer",
                "zygote",
                "gpu-process",
                "utility",
                "crashpad-handler",
            ]
            .contains(&kind)
            {
                return Err(format!("unsupported helper type: {kind}"));
            }
        }
        if argument == "--" {
            return Err("argument separator is not supported".into());
        }
    }
    let mut zygote_exception_seen = false;
    for argument in args {
        // Exception is EXACTLY one bare token with EXACTLY one canonical zygote
        // type. Never add this flag ourselves or exempt any other bypass.
        if argument == "--no-zygote-sandbox"
            && helper_type == Some("zygote")
            && !zygote_exception_seen
        {
            zygote_exception_seen = true;
            continue;
        }
        let key = argument.split('=').next().unwrap_or(argument);
        let normalized = key.trim_start_matches('-').to_ascii_lowercase();
        if [
            "no-sandbox",
            "disable-setuid-sandbox",
            "disable-seccomp-filter-sandbox",
            "disable-namespace-sandbox",
            "disable-gpu-sandbox",
            "no-zygote-sandbox",
            "single-process",
            "no-zygote",
            "disable-web-security",
            "ignore-certificate-errors",
        ]
        .contains(&normalized.as_str())
            || (normalized.contains("sandbox")
                && (normalized.starts_with("disable-") || normalized.starts_with("no-")))
        {
            return Err(format!("forbidden Chromium argument: {argument}"));
        }
    }
    if helper_type.is_some() {
        return Ok(Invocation::Helper);
    }
    Ok(Invocation::Browser)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Invocation, String> {
        parse_arguments(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn normal_cli_is_not_a_helper() {
        assert!(matches!(parse(&["--help"]), Ok(Invocation::Browser)));
    }
    #[test]
    fn only_exact_generated_zygote_exception() {
        assert_eq!(
            parse(&["--type=zygote", "--no-zygote-sandbox"]),
            Ok(Invocation::Helper)
        );
        for args in [
            vec!["--type=renderer", "--no-zygote-sandbox"],
            vec!["--type=zygote", "--no-zygote-sandbox=1"],
            vec![
                "--type=zygote",
                "--no-zygote-sandbox",
                "--no-zygote-sandbox",
            ],
            vec!["--type=zygote", "--type=renderer"],
            vec!["--Type=renderer"],
            vec!["--type", "renderer"],
            vec!["--type=renderer", "--NO-SANDBOX"],
            vec!["--disable-seccomp-filter-sandbox"],
            vec!["--type=other"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }
}

/// Dispatcher boundary: consume exactly one executable token, never a switch.
/// Reject a sliced argv so a future caller cannot silently drop the first flag.
pub fn classify_argv(argv: &[String]) -> Result<Invocation, String> {
    let executable = argv.first().ok_or("missing executable in full argv")?;
    if executable.is_empty() || executable.starts_with('-') {
        return Err("helper dispatcher requires full argv including executable".into());
    }
    parse_arguments(&argv[1..])
}

#[cfg(test)]
mod dispatcher_tests {
    use super::*;
    fn classify(argv: &[&str]) -> Result<Invocation, String> {
        classify_argv(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn full_argv_keeps_first_helper_switch() {
        assert_eq!(
            classify(&["/isolated/herdr-app", "--type=renderer"]),
            Ok(Invocation::Helper)
        );
        assert_eq!(
            classify(&["/isolated/herdr-app", "--help"]),
            Ok(Invocation::Browser)
        );
        assert!(classify(&["--type=renderer"]).is_err());
    }
    #[test]
    fn first_bypass_switch_is_never_dropped() {
        assert!(classify(&["/isolated/herdr-app", "--no-sandbox", "--type=renderer"]).is_err());
        assert!(classify(&["--no-sandbox", "--type=renderer"]).is_err());
        assert_eq!(
            classify(&[
                "/isolated/herdr-app",
                "--no-zygote-sandbox",
                "--type=zygote"
            ]),
            Ok(Invocation::Helper)
        );
        for argv in [
            vec!["exe", "--no-zygote-sandbox", "--type=renderer"],
            vec!["exe", "--no-zygote-sandbox=1", "--type=zygote"],
            vec![
                "exe",
                "--no-zygote-sandbox",
                "--type=zygote",
                "--no-zygote-sandbox",
            ],
            vec![
                "exe",
                "--no-zygote-sandbox",
                "--type=zygote",
                "--no-sandbox",
            ],
        ] {
            assert!(classify(&argv).is_err(), "{argv:?}");
        }
    }
}
