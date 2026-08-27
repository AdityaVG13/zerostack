# fszero bash completion (fszero-x7n7.4 / R-IDEA-012)
# Install: eval "$(fszero completions bash)"
# Or: source packaging/completions/fszero.bash

_fszero_completions() {
  local cur prev
  COMPREPLY=()
  cur="${COMP_WORDS[COMP_CWORD]}"
  prev="${COMP_WORDS[COMP_CWORD-1]}"
  local verbs="help install uninstall sbom doctor serve batch migrate-cas telemetry zeroref-fixture codemode capabilities catalog tools layout robot-triage robot-docs completions"
  local flags="--help --json --root --prefix --surface --binary --dry-run --yes --mode=mcp --mode=codemode --serve --supervise --raw-worker --telemetry --no-telemetry"

  if [[ ${COMP_CWORD} -eq 1 ]]; then
    COMPREPLY=( $(compgen -W "${verbs} ${flags}" -- "${cur}") )
    return 0
  fi

  case "${prev}" in
    --surface)
      COMPREPLY=( $(compgen -W "mcp codemode" -- "${cur}") )
      return 0
      ;;
    completions)
      COMPREPLY=( $(compgen -W "bash zsh fish" -- "${cur}") )
      return 0
      ;;
    --root|--prefix|--binary)
      COMPREPLY=( $(compgen -f -- "${cur}") )
      return 0
      ;;
  esac

  COMPREPLY=( $(compgen -W "${flags} ${verbs}" -- "${cur}") )
}

complete -F _fszero_completions fszero
complete -F _fszero_completions fszero-mcp
complete -F _fszero_completions fszero-codemode
