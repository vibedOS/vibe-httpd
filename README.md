# vibe-httpd

A statically linked, libc-free HTTP/1.1 server for vibeOS.

```sh
cargo build --release
target/x86_64-unknown-linux-gnu/release/vibe-httpd 8080 ./index.html
```

It serves `/` and `/health` through two restartable workers with bounded
4 KiB request headers. The index file is capped at 16 KiB.

MIT licensed.
