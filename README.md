# vibe-httpd

A statically linked, libc-free HTTP/1.1 server for vibeOS.

```sh
cargo build --release
target/x86_64-unknown-linux-gnu/release/vibe-httpd 8080
```

The first version serves `/`, `/health`, and bounded 4 KiB request headers.

MIT licensed.
