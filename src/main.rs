// SPDX-License-Identifier: MIT

#![no_main]
#![no_std]

use core::panic::PanicInfo;
use vibe_rt::{
    Args, Env, Errno, Fork, Result, accept, close, entry, eprintln, fork, getpid, getppid, read,
    sleep, tcp_listener, terminate_with_parent, wait_any, write_all,
};

const WORKER_COUNT: usize = 2;
const INDEX: &[u8] = b"HTTP/1.1 200 OK\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 23\r\n\
Connection: close\r\n\
Server: vibe-httpd/0.1\r\n\
\r\n\
vibeOS is serving HTTP\n";
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
                *worker = spawn_worker(listener, master);
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

fn spawn_worker(listener: i32, master: i32) -> Option<i32> {
    match fork() {
        Ok(Fork::Parent(pid)) => Some(pid),
        Ok(Fork::Child) => {
            if terminate_with_parent().is_err() || getppid() != master {
                vibe_rt::exit(1);
            }
            worker(listener)
        }
        Err(error) => {
            eprintln!("vibe-httpd: fork failed: errno {}", error.0);
            None
        }
    }
}

fn worker(listener: i32) -> ! {
    loop {
        match accept(listener) {
            Ok(connection) => {
                serve(connection);
                let _ = close(connection);
            }
            Err(error) => eprintln!("vibe-httpd: accept failed: errno {}", error.0),
        }
    }
}

fn serve(connection: i32) {
    let mut request = [0_u8; 4096];
    let response = match read_request(connection, &mut request) {
        Ok(length) => route(&request[..length]),
        Err(_) => BAD_REQUEST,
    };
    let _ = write_all(connection as usize, response);
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

fn route(request: &[u8]) -> &'static [u8] {
    let Some(end) = request.windows(2).position(|bytes| bytes == b"\r\n") else {
        return BAD_REQUEST;
    };
    match &request[..end] {
        b"GET / HTTP/1.1" | b"GET / HTTP/1.0" => INDEX,
        b"GET /health HTTP/1.1" | b"GET /health HTTP/1.0" => HEALTH,
        line if line.starts_with(b"GET ") => NOT_FOUND,
        _ => METHOD_NOT_ALLOWED,
    }
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
