// SPDX-License-Identifier: MIT

#![no_main]
#![no_std]

use core::ffi::CStr;
use core::panic::PanicInfo;
use vibe_rt::{
    Args, Env, Errno, Fork, Result, accept, close, entry, eprintln, fork, getpid, getppid,
    open_read, read, set_read_timeout, sleep, tcp_listener, terminate_with_parent, wait_any,
    write_all,
};

const WORKER_COUNT: usize = 2;
const MAX_REQUESTS: usize = 16;
const HEALTH: Response = Response::new(b"200 OK", b"text/plain; charset=utf-8", b"ok\n");
const NOT_FOUND: Response = Response::new(
    b"404 Not Found",
    b"text/plain; charset=utf-8",
    b"not found\n",
);
const METHOD_NOT_ALLOWED: Response = Response {
    status: b"405 Method Not Allowed",
    content_type: b"text/plain; charset=utf-8",
    extra_header: b"Allow: GET\r\n",
    body: b"method not allowed\n",
};
const BAD_REQUEST: Response = Response::new(
    b"400 Bad Request",
    b"text/plain; charset=utf-8",
    b"bad request\n",
);
const SERVER_ERROR: Response = Response::new(
    b"500 Internal Server Error",
    b"text/plain; charset=utf-8",
    b"server error\n",
);

struct Response {
    status: &'static [u8],
    content_type: &'static [u8],
    extra_header: &'static [u8],
    body: &'static [u8],
}

impl Response {
    const fn new(status: &'static [u8], content_type: &'static [u8], body: &'static [u8]) -> Self {
        Self {
            status,
            content_type,
            extra_header: b"",
            body,
        }
    }
}

entry!(main);

fn main(mut args: Args<'_>, _env: Env<'_>) -> i32 {
    let _program = args.next();
    let port = match args.next() {
        Some(value) => match parse_port(value) {
            Some(port) => port,
            None => {
                eprintln!("usage: vibe-httpd [PORT]");
                return 2;
            }
        },
        None => 8080,
    };
    let mut index_storage = [0_u8; 512];
    let index = match args.next() {
        Some(path) => match argument_path(path, &mut index_storage) {
            Some(path) => path,
            None => {
                eprintln!("vibe-httpd: index path too long");
                return 2;
            }
        },
        None => c"/var/www/index.html",
    };
    if args.next().is_some() {
        eprintln!("usage: vibe-httpd [PORT] [INDEX]");
        return 2;
    }

    let listener = match tcp_listener(port) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("vibe-httpd: listen failed: errno {}", error.0);
            return 1;
        }
    };
    vibe_rt::println!("vibe-httpd 0.1 listening on 0.0.0.0:{port}");

    let master = getpid();
    let mut workers = [None; WORKER_COUNT];
    loop {
        for worker in &mut workers {
            if worker.is_none() {
                *worker = spawn_worker(listener, master, index);
            }
        }
        if workers.iter().any(Option::is_none) {
            sleep(1);
            continue;
        }

        match wait_any() {
            Ok((pid, _status)) => {
                if let Some(worker) = workers.iter_mut().find(|worker| **worker == Some(pid)) {
                    *worker = None;
                }
            }
            Err(error) => {
                eprintln!("vibe-httpd: wait failed: errno {}", error.0);
                sleep(1);
            }
        }
    }
}

fn spawn_worker(listener: i32, master: i32, index: &CStr) -> Option<i32> {
    match fork() {
        Ok(Fork::Parent(pid)) => Some(pid),
        Ok(Fork::Child) => {
            if terminate_with_parent().is_err() || getppid() != master {
                vibe_rt::exit(1);
            }
            worker(listener, index)
        }
        Err(error) => {
            eprintln!("vibe-httpd: fork failed: errno {}", error.0);
            None
        }
    }
}

fn worker(listener: i32, index: &CStr) -> ! {
    loop {
        match accept(listener) {
            Ok(connection) => {
                if set_read_timeout(connection, 5).is_ok() {
                    serve(connection, index);
                }
                let _ = close(connection);
            }
            Err(error) => eprintln!("vibe-httpd: accept failed: errno {}", error.0),
        }
    }
}

fn serve(connection: i32, index: &CStr) {
    let mut request = [0_u8; 4096];
    let mut buffered = 0;
    for count in 0..MAX_REQUESTS {
        let length = match read_request(connection, &mut request, &mut buffered) {
            Ok(length) => length,
            Err(_) => {
                send_response(connection, &BAD_REQUEST, false);
                return;
            }
        };
        let keep_alive = request_keep_alive(&request[..length]) && count + 1 < MAX_REQUESTS;
        match route(&request[..length]) {
            Route::Index => serve_index(connection, index, keep_alive),
            Route::Fixed(response) => send_response(connection, response, keep_alive),
        }

        request.copy_within(length..buffered, 0);
        buffered -= length;
        if !keep_alive {
            return;
        }
    }
}

fn read_request(connection: i32, buffer: &mut [u8], length: &mut usize) -> Result<usize> {
    loop {
        if let Some(end) = buffer[..*length]
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
        {
            return Ok(end + 4);
        }
        if *length == buffer.len() {
            return Err(Errno(7));
        }
        let count = read(connection as usize, &mut buffer[*length..])?;
        if count == 0 {
            return Err(Errno(71));
        }
        *length += count;
    }
}

enum Route {
    Index,
    Fixed(&'static Response),
}

fn route(request: &[u8]) -> Route {
    let Some(end) = request.windows(2).position(|bytes| bytes == b"\r\n") else {
        return Route::Fixed(&BAD_REQUEST);
    };
    match &request[..end] {
        b"GET / HTTP/1.1" | b"GET / HTTP/1.0" => Route::Index,
        b"GET /health HTTP/1.1" | b"GET /health HTTP/1.0" => Route::Fixed(&HEALTH),
        line if line.starts_with(b"GET ") => Route::Fixed(&NOT_FOUND),
        _ => Route::Fixed(&METHOD_NOT_ALLOWED),
    }
}

fn request_keep_alive(request: &[u8]) -> bool {
    let mut lines = request.split(|byte| *byte == b'\n');
    let default = lines
        .next()
        .is_some_and(|line| trim_ascii(line).ends_with(b" HTTP/1.1"));
    let mut keep_alive = false;

    for line in lines {
        let Some(separator) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if !trim_ascii(&line[..separator]).eq_ignore_ascii_case(b"connection") {
            continue;
        }
        for token in line[separator + 1..].split(|byte| *byte == b',') {
            let token = trim_ascii(token);
            if token.eq_ignore_ascii_case(b"close") {
                return false;
            }
            keep_alive |= token.eq_ignore_ascii_case(b"keep-alive");
        }
    }
    default || keep_alive
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

fn serve_index(connection: i32, path: &CStr, keep_alive: bool) {
    let file = match open_read(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("vibe-httpd: open index failed: errno {}", error.0);
            send_response(connection, &NOT_FOUND, keep_alive);
            return;
        }
    };
    // ponytail: a fixed page cap avoids an allocator; stream files when pages need to exceed 16 KiB.
    let mut content = [0_u8; 16 * 1024];
    let mut length = 0;
    loop {
        if length == content.len() {
            let mut extra = [0_u8; 1];
            if read(file as usize, &mut extra) != Ok(0) {
                let _ = close(file);
                send_response(connection, &SERVER_ERROR, keep_alive);
                return;
            }
            break;
        }
        match read(file as usize, &mut content[length..]) {
            Ok(0) => break,
            Ok(count) => length += count,
            Err(error) => {
                eprintln!("vibe-httpd: read index failed: errno {}", error.0);
                let _ = close(file);
                send_response(connection, &SERVER_ERROR, keep_alive);
                return;
            }
        }
    }
    let _ = close(file);

    send_parts(
        connection,
        b"200 OK",
        b"text/html; charset=utf-8",
        b"",
        &content[..length],
        keep_alive,
    );
}

fn send_response(connection: i32, response: &Response, keep_alive: bool) {
    send_parts(
        connection,
        response.status,
        response.content_type,
        response.extra_header,
        response.body,
        keep_alive,
    );
}

fn send_parts(
    connection: i32,
    status: &[u8],
    content_type: &[u8],
    extra_header: &[u8],
    body: &[u8],
    keep_alive: bool,
) {
    let connection = connection as usize;
    let _ = write_all(connection, b"HTTP/1.1 ");
    let _ = write_all(connection, status);
    let _ = write_all(connection, b"\r\nContent-Type: ");
    let _ = write_all(connection, content_type);
    let _ = write_all(connection, b"\r\nContent-Length: ");
    write_number(connection, body.len());
    let _ = write_all(connection, b"\r\n");
    let _ = write_all(connection, extra_header);
    let _ = write_all(connection, b"Connection: ");
    let _ = write_all(
        connection,
        if keep_alive { b"keep-alive" } else { b"close" },
    );
    let _ = write_all(connection, b"\r\nServer: vibe-httpd/0.1\r\n\r\n");
    let _ = write_all(connection, body);
}

fn write_number(fd: usize, mut value: usize) {
    let mut digits = [0_u8; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        digits[start] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let _ = write_all(fd, &digits[start..]);
}

fn argument_path<'a>(value: &[u8], storage: &'a mut [u8]) -> Option<&'a CStr> {
    if value.len() >= storage.len() {
        return None;
    }
    storage[..value.len()].copy_from_slice(value);
    storage[value.len()] = 0;
    CStr::from_bytes_with_nul(&storage[..=value.len()]).ok()
}

fn parse_port(value: &[u8]) -> Option<u16> {
    if value.is_empty() {
        return None;
    }
    let mut port = 0_u16;
    for byte in value {
        let digit = byte.checked_sub(b'0')?;
        if digit > 9 {
            return None;
        }
        port = port.checked_mul(10)?.checked_add(digit as u16)?;
    }
    (port != 0).then_some(port)
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    eprintln!("vibe-httpd panic: {info}");
    vibe_rt::exit(101)
}
