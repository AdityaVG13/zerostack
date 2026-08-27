//! Shell completion scripts for the clap-less fszero shim (fszero-x7n7.4 / R-IDEA-012).
//!
//! Generated from [`super::SHIM_COMMANDS`] + [`super::COMMON_FLAGS`] so the
//! script stays aligned with did-you-mean dictionaries.

use super::{COMMON_FLAGS, SHIM_COMMANDS};

/// Supported shells for `fszero completions <shell>`.
pub const COMPLETION_SHELLS: &[&str] = &["bash", "zsh", "fish"];

/// Render a completion script for `shell` (`bash` | `zsh` | `fish`).
pub fn completion_script(shell: &str) -> Result<String, String> {
    let s = shell.trim().to_ascii_lowercase();
    match s.as_str() {
        "bash" => Ok(bash_script()),
        "zsh" => Ok(zsh_script()),
        "fish" => Ok(fish_script()),
        other => Err(format!(
            "unknown shell {other:?}; use one of: {}",
            COMPLETION_SHELLS.join(", ")
        )),
    }
}

fn verbs_space() -> String {
    SHIM_COMMANDS.join(" ")
}

fn flags_space() -> String {
    COMMON_FLAGS.join(" ")
}

fn bash_script() -> String {
    let verbs = verbs_space();
    let flags = flags_space();
    format!(
        r#"# fszero bash completion (fszero-x7n7.4 / R-IDEA-012)
# Install: eval "$(fszero completions bash)"
# Or: source packaging/completions/fszero.bash

_fszero_completions() {{
  local cur prev
  COMPREPLY=()
  cur="${{COMP_WORDS[COMP_CWORD]}}"
  prev="${{COMP_WORDS[COMP_CWORD-1]}}"
  local verbs="{verbs}"
  local flags="{flags}"

  if [[ ${{COMP_CWORD}} -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "${{verbs}} ${{flags}}" -- "${{cur}}") )
    return 0
  fi

  case "${{prev}}" in
    --surface)
      COMPREPLY=( $(compgen -W "mcp codemode" -- "${{cur}}") )
      return 0
      ;;
    --mode|--mode=*)
      COMPREPLY=( $(compgen -W "mcp codemode" -- "${{cur}}") )
      return 0
      ;;
    completions)
      COMPREPLY=( $(compgen -W "bash zsh fish" -- "${{cur}}") )
      return 0
      ;;
    --root|--prefix|--binary)
      COMPREPLY=( $(compgen -f -- "${{cur}}") )
      return 0
      ;;
  esac

  COMPREPLY=( $(compgen -W "${{flags}} ${{verbs}}" -- "${{cur}}") )
}}

complete -F _fszero_completions fszero
complete -F _fszero_completions fszero-mcp
complete -F _fszero_completions fszero-codemode
"#
    )
}

fn zsh_script() -> String {
    let verbs = verbs_space();
    let flags = flags_space();
    format!(
        r#"# fszero zsh completion (fszero-x7n7.4 / R-IDEA-012)
# Install: eval "$(fszero completions zsh)"
# Or: source packaging/completions/fszero.zsh

#compdef fszero fszero-mcp fszero-codemode

_fszero() {{
  local -a verbs flags
  verbs=({verbs})
  flags=({flags})
  local context state state_descr line
  typeset -A opt_args

  _arguments -C \
    '1:command:->cmds' \
    '*::arg:->args'

  case $state in
    cmds)
      _describe -t commands 'fszero command' verbs
      _describe -t options 'fszero flag' flags
      ;;
    args)
      case $words[1] in
        completions)
          _values 'shell' bash zsh fish
          ;;
        install|uninstall|sbom)
          _arguments \
            '--surface[mcp|codemode]:surface:(mcp codemode)' \
            '--prefix[install prefix]:dir:_files -/' \
            '--binary[binary path]:file:_files' \
            '--dry-run[print plan]' \
            '--yes[confirm mutation]' \
            '--json[JSON output]'
          ;;
        doctor|capabilities|layout|robot-triage|robot-docs)
          _arguments '--json[JSON output]' '--root[workspace root]:dir:_files -/'
          ;;
        *)
          _arguments \
            '--help[help]' \
            '--json[JSON output]' \
            '--root[workspace root]:dir:_files -/' \
            '--dry-run[print plan]' \
            '--yes[confirm mutation]'
          ;;
      esac
      ;;
  esac
}}

_fszero "$@"
"#
    )
}

fn fish_script() -> String {
    let mut out = String::from(
        "# fszero fish completion (fszero-x7n7.4 / R-IDEA-012)\n# Install: fszero completions fish | source\n# Or: source packaging/completions/fszero.fish\n\n",
    );
    for v in SHIM_COMMANDS {
        out.push_str(&format!(
            "complete -c fszero -n '__fish_use_subcommand' -a '{v}' -d 'fszero {v}'\n"
        ));
        out.push_str(&format!(
            "complete -c fszero-mcp -n '__fish_use_subcommand' -a '{v}' -d 'fszero {v}'\n"
        ));
        out.push_str(&format!(
            "complete -c fszero-codemode -n '__fish_use_subcommand' -a '{v}' -d 'fszero {v}'\n"
        ));
    }
    for f in COMMON_FLAGS {
        let short = f.trim_start_matches('-');
        out.push_str(&format!(
            "complete -c fszero -l '{short}' -d 'fszero flag'\n"
        ));
    }
    out.push_str(
        "complete -c fszero -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish'\n",
    );
    out.push_str(
        "complete -c fszero -n '__fish_seen_subcommand_from install sbom' -l surface -a 'mcp codemode'\n",
    );
    out
}

#[cfg(test)]
#[path = "../../../../../tests/fszero/unit/fs-zero/completions_tests.rs"]
mod tests;
