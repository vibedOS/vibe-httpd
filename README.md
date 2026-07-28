# vibe-httpd

`vibe-httpd` is the small, libc-free HTTP/1.1 server written for vibeOS. It
serves the persistent vibeOS index page and a health endpoint without an
allocator or an external runtime.

## Usage

```text
vibe-httpd [PORT] [INDEX]
```

The defaults are port `8080` and `/var/www/index.html`.

On a Linux development host:

```sh
cargo build --release
printf '<h1>hello</h1>\n' > /tmp/vibe-index.html
target/x86_64-unknown-linux-gnu/release/vibe-httpd 8080 /tmp/vibe-index.html
```

Then request <http://127.0.0.1:8080/> or
<http://127.0.0.1:8080/health>.

## Routes

| Method and path | Response |
| --- | --- |
| `GET /` | configured HTML index |
| `HEAD /` | index headers without a body |
| `GET /health` | `200 OK` with `ok` |
| `HEAD /health` | health headers without a body |
| other `GET` or `HEAD` paths | `404 Not Found` |
| other methods | `405 Method Not Allowed` |

HTTP/1.0 and HTTP/1.1 request lines are accepted. HTTP/1.1 connections are
kept alive by default; HTTP/1.0 connections require `Connection: keep-alive`.

## Runtime bounds

- two supervised worker processes
- 4 KiB maximum request headers
- 16 requests per connection
- five-second connection read timeout
- 16 KiB maximum index file
- listen backlog of 16

These fixed limits keep the implementation allocator-free. TLS, request
bodies, directory serving, MIME detection, and access logs are not currently
implemented.

## Build

Rust 1.94.0 is selected by `rust-toolchain.toml`.

```sh
cargo build --release
```

The release binary is statically linked and does not use libc.

## License

MIT
