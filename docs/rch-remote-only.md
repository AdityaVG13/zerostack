# Remote-only compilation with RCH

Use the versioned wrapper for every Rust or C build, test, check, lint, and compiler command in the ZeroStack/pi workflow:

~~~sh
python3 scripts/rch_remote.py -- cargo test --workspace
python3 scripts/rch_remote.py -- cargo clippy --workspace --all-targets
python3 scripts/rch_remote.py -- cc -c src/example.c -o target/example.o
~~~

The wrapper executes argv directly, never through a shell. Before rch exec, it resolves the current directory and RCH configured canonical root. Outside-root work is rejected with both resolved paths and a copyable git worktree remedy under that root.

## Fail-closed contract

Wrapper version 1 sets RCH_FORCE_REMOTE=true, RCH_REQUIRE_REMOTE=true, RCH_QUEUE_WHEN_BUSY=true, and RCH_VISIBILITY=summary before RCH starts. An explicit verbose visibility setting is stronger and remains unchanged.

Current RCH native behavior distinguishes force from proof: RCH_FORCE_REMOTE requests remote selection, but older or misconfigured paths can still print [RCH] local and return success. Current RCH supports RCH_REQUIRE_REMOTE for fail-closed operation and RCH_QUEUE_WHEN_BUSY for contention. The wrapper applies both, checks telemetry, and returns nonzero on [RCH] local or successful compilation without [RCH] remote proof.

The final stderr line classifies remote_success, queued_or_busy, forbidden_local_fallback, remote_failure, or configuration_error. Remote command failures preserve their exit status. Wrapper-detected fallback, busy-with-zero-status, and configuration errors return 70, 75, and 78.

~~~sh
python3 scripts/rch_remote.py --wrapper-version
~~~
