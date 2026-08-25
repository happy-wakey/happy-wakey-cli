# happy-wakey-cli

A small Rust command-line client for Happy Wakey's Shared Auth boundary. It uses the official `shared-auth-client` crate pinned to an immutable reviewed commit; it does not recreate token validation or redirect policy locally.

The CLI supports public capability discovery and user-token verification. Access tokens are accepted only through `HAPPY_WAKEY_ACCESS_TOKEN`, never a command-line flag, URL, log field, or output. Product authorization remains in Happy Wakey services; a successful authentication check is not permission to access an alarm.

## Use

Use the released `zed-pkg` CLI for dependency resolution, installation, and scripts:

```sh
zed validate
zed install --adapter rust
zed run cargo test --locked
```

Then run against the customer Shared Auth realm:

```sh
HAPPY_WAKEY_SHARED_AUTH_BASE=https://auth.example.test \
cargo run -- capabilities

HAPPY_WAKEY_SHARED_AUTH_BASE=https://auth.example.test \
HAPPY_WAKEY_ACCESS_TOKEN='<runtime-injected-token>' \
cargo run -- verify
```

Remote authorities must use HTTPS. Never reuse a user token as the independent service credential for protected introspection, and never ship an introspection credential to this CLI.
