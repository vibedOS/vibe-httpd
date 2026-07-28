# vibe-httpd

A statically linked, libc-free HTTP/1.1 server for vibeOS.

```sh
cargo build --release
target/x86_64-unknown-linux-gnu/release/vibe-httpd 8080 ./index.html
```

It serves `GET` and `HEAD` for `/` and `/health` through two restartable workers with bounded
4 KiB request headers. Connections time out after five idle seconds and can
carry up to 16 requests. The index file is capped at 16 KiB.

MIT licensed.
