# FSZero man page

The checked-in page documents the standalone operator CLI.

## Pages

| File | Section | Topic |
| --- | --- | --- |
| `fszero.1` | 1 | standalone operator commands, environment, and exit status |

## Preview (no install)

```bash
man ./packaging/man/fszero.1
# or
nroff -man packaging/man/fszero.1 | less
```

## Install (optional)

```bash
# user-local
mkdir -p ~/.local/share/man/man1
cp packaging/man/*.1 ~/.local/share/man/man1/
# ensure manpath includes ~/.local/share/man
man fszero
```

System-wide (example):

```bash
sudo cp packaging/man/*.1 /usr/local/share/man/man1/
sudo mandb 2>/dev/null || true
```

## Regenerate / lint

```bash
python3 packaging/man/generate_fszero_1.py --check   # structure gate
# --write refreshes NAME/COMMANDS tables from the embedded robot-docs mirror
```

Source of truth: `crates/fs-zero/src/packaging/mod.rs` and the canonical
architecture and installation guides under `docs/`.
