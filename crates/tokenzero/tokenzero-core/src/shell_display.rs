pub fn shell_display_command_from_argv(argv: &[String]) -> String {
    shell_display_command_from_argv_for_platform(argv, "posix")
}

pub fn shell_display_command_from_argv_for_platform(argv: &[String], platform: &str) -> String {
    argv.iter()
        .map(|arg| shell_display_arg(arg, platform))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn shell_display_arg(arg: &str, platform: &str) -> String {
    let (quote, needle, replacement, safe) = match platform {
        "cmd" | "windows" => ('"', '"', "\"\"", "-_./:,=@\\"),
        "powershell" | "pwsh" => ('\'', '\'', "''", "-_./:,=@\\"),
        _ => ('\'', '\'', "'\\''", "-_./:,=@%"),
    };
    if !arg.is_empty()
        && arg
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || safe.contains(ch))
    {
        arg.to_string()
    } else {
        format!("{quote}{}{quote}", arg.replace(needle, replacement))
    }
}
