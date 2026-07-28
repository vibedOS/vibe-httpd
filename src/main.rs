// SPDX-License-Identifier: MIT

#![no_main]
#![no_std]

use core::ffi::CStr;
use core::panic::PanicInfo;
use vibe_rt::{
    Args, Env, Errno, Fork, Result, accept, close, entry, eprintln, fork, getpid, getppid,
    open_read, read, sleep, tcp_listener, terminate_with_parent, wait_any, write_all,
};

const WORKER_COUNT: usize = 2;
const INDEX_HEADER: &[u8] = b"HTTP/1.1 200 OK\r\n\
Content-Type: text/html; charset=utf-8\r\n\
Content-Length: ";
const INDEX_HEADER_END: &[u8] = b"\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n";
const HEALTH: &[u8] = b"HTTP/1.1 200 OK\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 3\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n\
ok\n";
const NOT_FOUND: &[u8] = b"HTTP/1.1 404 Not Found\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 10\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n\
not found\n";
const METHOD_NOT_ALLOWED: &[u8] = b"HTTP/1.1 405 Method Not Allowed\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 19\r\n\
Allow: GET\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n\
method not allowed\n";
const BAD_REQUEST: &[u8] = b"HTTP/1.1 400 Bad Request\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 12\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n\
bad request\n";
const SERVER_ERROR: &[u8] = b"HTTP/1.1 500 Internal Server Error\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 13\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n\
server error\n";

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
                serve(connection, index);
                let _ = close(connection);
            }
            Err(error) => eprintln!("vibe-httpd: accept failed: errno {}", error.0),
        }
    }
}

fn serve(connection: i32, index: &CStr) {
    let mut request = [0_u8; 4096];
    let route = match read_request(connection, &mut request) {
        Ok(length) => route(&request[..length]),
        Err(_) => Route::Fixed(BAD_REQUEST),
    };
    match route {
        Route::Index => serve_index(connection, index),
        Route::Fixed(response) => {
            let _ = write_all(connection as usize, response);
        }
    }
}

fn read_request(connection: i32, buffer: &mut [u8]) -> Result<usize> {
    let mut length = 0;
    loop {
        if length == buffer.len() {
            return Err(Errno(7));
        }
        let count = read(connection as usize, &mut buffer[length..])?;
        if count == 0 {
            return Err(Errno(71));
        }
        length += count;
        if buffer[..length]
            .windows(4)
            .any(|bytes| bytes == b"\r\n\r\n")
        {
            return Ok(length);
        }
    }
}

enum Route {
    Index,
    Fixed(&'static [u8]),
}

fn route(request: &[u8]) -> Route {
    let Some(end) = request.windows(2).position(|bytes| bytes == b"\r\n") else {
        return Route::Fixed(BAD_REQUEST);
    };
    match &request[..end] {
        b"GET / HTTP/1.1" | b"GET / HTTP/1.0" => Route::Index,
        b"GET /health HTTP/1.1" | b"GET /health HTTP/1.0" => Route::Fixed(HEALTH),
        line if line.starts_with(b"GET ") => Route::Fixed(NOT_FOUND),
        _ => Route::Fixed(METHOD_NOT_ALLOWED),
    }
}

fn serve_index(connection: i32, path: &CStr) {
    let file = match open_read(path) {
        Ok(file) => file,
        Err(error) => {
            eprintln!("vibe-httpd: open index failed: errno {}", error.0);
            let _ = write_all(connection as usize, NOT_FOUND);
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
                let _ = write_all(connection as usize, SERVER_ERROR);
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
                let _ = write_all(connection as usize, SERVER_ERROR);
                return;
            }
        }
    }
    let _ = close(file);

    let connection = connection as usize;
    let _ = write_all(connection, INDEX_HEADER);
    write_number(connection, length);
    let _ = write_all(connection, INDEX_HEADER_END);
    let _ = write_all(connection, &content[..length]);
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
