# fszero zsh completion (fszero-x7n7.4)
# Install: eval "$(fszero completions zsh)"
#compdef fszero fszero-mcp fszero-codemode
_fszero() {
  local -a verbs flags
  verbs=(help install uninstall sbom doctor serve batch migrate-cas telemetry zeroref-fixture codemode capabilities catalog tools layout robot-triage robot-docs completions)
  flags=(--help --json --root --prefix --surface --binary --dry-run --yes --mode=mcp --mode=codemode --serve --supervise --raw-worker --telemetry --no-telemetry)
  _describe 'command' verbs
  _describe 'flag' flags
}
_fszero "$@"
