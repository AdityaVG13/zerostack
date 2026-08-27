# fszero shell completions (R-IDEA-012 / fszero-x7n7.4)

Clap-less scripts for top shim verbs (`SHIM_COMMANDS`) and common flags.

## Install

```bash
# preferred: live generator (stays aligned with SHIM_COMMANDS)
eval "$(fszero completions bash)"   # bash
eval "$(fszero completions zsh)"    # zsh
fszero completions fish | source    # fish
```

Or source the checked-in scripts:

```bash
source packaging/completions/fszero.bash
# zsh: source packaging/completions/fszero.zsh
# fish: source packaging/completions/fszero.fish
```

Generator implementation: `src/packaging/completions.rs`.
