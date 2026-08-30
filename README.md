# happy-wakey-cli

A production-oriented Rust command-line client for Shared Auth verification and
the Happy Wakey alarm API. It uses the canonical versioned Shared Auth HTTPS
endpoints with redirects disabled and bounded streaming responses, and imports
all alarm request/response bodies from `happy-wakey-interfaces`.

The command boundary is parsed exclusively by `flags-2-env`. Command handlers
compose typed request builders and bounded HTTP adapters; URLs, alarm payloads,
and lifecycle transitions are validated by pure functions before effects run.
Ores telemetry records only a closed command classification and success/failure
bit—never argv, bearer tokens, request bodies, or response bodies.

The CLI supports public capability discovery, credential verification,
subject-scoped alarm listing and creation, and generation-fenced occurrence
transitions. Access tokens are accepted only through
`HAPPY_WAKEY_ACCESS_TOKEN`, never a command-line flag, URL, log field, or
output. Product authorization remains in Happy Wakey services; a successful
authentication check is not permission to access an alarm.

## Use

Use the released `zed-pkg` CLI for dependency resolution, installation, and scripts:

```sh
zed validate
zed install --adapter rust
zed run cargo test --locked
```

The argument parser is the bundled Rust package from canonical
`flags-2-env/flags-2-env` commit
`b07214e30b4da675a0f591e362a2039cb47e9055`. `.cli-flags.toml` is the single
flag and subcommand schema; unknown or invalid arguments fail without echoing
their values. Package the schema beside the executable.

Run against the customer Shared Auth realm:

```sh
HAPPY_WAKEY_SHARED_AUTH_BASE=https://auth.example.test \
cargo run -- capabilities

HAPPY_WAKEY_SHARED_AUTH_BASE=https://auth.example.test \
HAPPY_WAKEY_ACCESS_TOKEN='<runtime-injected-token>' \
cargo run -- verify

HAPPY_WAKEY_API_BASE=https://api.example.test \
HAPPY_WAKEY_ACCESS_TOKEN='<runtime-injected-token>' \
cargo run -- alarms list --pretty

HAPPY_WAKEY_API_BASE=https://api.example.test \
HAPPY_WAKEY_ACCESS_TOKEN='<runtime-injected-token>' \
cargo run -- alarms create \
  --label 'Weekday wake-up' \
  --local-time 07:30 \
  --time-zone America/Chicago \
  --weekdays '[1,2,3,4,5]' \
  --sound bell \
  --volume 0.8

HAPPY_WAKEY_API_BASE=https://api.example.test \
HAPPY_WAKEY_ACCESS_TOKEN='<runtime-injected-token>' \
cargo run -- occurrences transition \
  --occurrence-id '<uuid>' \
  --expected-generation 3 \
  --event snooze \
  --snooze-until 2026-08-25T12:15:00Z
```

Remote authorities must be credential-free root HTTPS URLs. The CLI never
accepts an introspection service credential: protected introspection belongs in
the API/web server trust boundary, while this client uses the customer token
only for verify and product API requests. Responses are JSON on stdout; errors
go to stderr so scripts receive a clean machine-readable channel.
