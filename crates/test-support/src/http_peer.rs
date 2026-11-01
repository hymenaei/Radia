use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use url::Url;

pub struct LoopbackPeer {
    pub url: Url,
    thread: thread::JoinHandle<Vec<Vec<u8>>>,
    stop_address: Option<SocketAddr>,
    response_bytes: Option<Arc<AtomicUsize>>,
}

impl LoopbackPeer {
    pub fn new(count: usize, mut respond: impl FnMut(usize, &[u8], &Url) -> (u16, Vec<u8>) + Send + 'static) -> Self {
        Self::new_with_headers(count, move |index, request, base| {
            let (status, body) = respond(index, request, base);
            (status, Vec::new(), body)
        })
    }

    pub fn new_with_headers(
        count: usize,
        mut respond: impl FnMut(usize, &[u8], &Url) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        listener.set_nonblocking(true).expect("set loopback listener nonblocking");
        let url = Url::parse(&format!("http://{}/", listener.local_addr().expect("read loopback address"))).expect("build loopback URL");
        let base = url.clone();
        let thread = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..count {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                            thread::sleep(Duration::from_millis(1))
                        }
                        Err(error) => panic!("expected loopback request: {error}"),
                    }
                };
                let request = read_request(&mut stream).expect("read loopback request");
                let (status, headers, body) = respond(index, &request, &base);
                write_response(&mut stream, status, headers, &body);
                requests.push(request);
            }
            requests
        });
        Self { url, thread, stop_address: None, response_bytes: None }
    }

    pub fn new_parallel(
        delay: Duration,
        respond: impl Fn(&[u8], &Url) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + Sync + 'static,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
        let address = listener.local_addr().expect("read loopback address");
        let url = Url::parse(&format!("http://{address}/")).expect("build loopback URL");
        let base = url.clone();
        let requests = Arc::new(Mutex::new(Vec::<(usize, Vec<u8>)>::new()));
        let peer_requests = Arc::clone(&requests);
        let response_bytes = Arc::new(AtomicUsize::new(0));
        let peer_response_bytes = Arc::clone(&response_bytes);
        let respond = Arc::new(respond);
        let thread = thread::spawn(move || {
            let mut workers = Vec::new();
            let mut index = 0;
            loop {
                let (mut stream, _) = listener.accept().expect("accept loopback request");
                let Some(request) = read_request_if_connected(&mut stream) else { continue };
                if String::from_utf8_lossy(&request).to_ascii_lowercase().starts_with("get /__stop ") {
                    break;
                }
                peer_requests.lock().expect("lock loopback request list").push((index, request.clone()));
                index += 1;
                let (status, headers, body) = respond(&request, &base);
                let peer_response_bytes = Arc::clone(&peer_response_bytes);
                workers.push(thread::spawn(move || {
                    thread::sleep(delay);
                    if write_response(&mut stream, status, headers, &body) {
                        peer_response_bytes.fetch_add(body.len(), Ordering::Relaxed);
                    }
                }));
            }
            for worker in workers {
                worker.join().expect("loopback response thread completes");
            }
            let mut requests = requests.lock().expect("lock loopback request list");
            requests.sort_unstable_by_key(|(index, _)| *index);
            requests.iter().map(|(_, request)| request.clone()).collect()
        });
        Self { url, thread, stop_address: Some(address), response_bytes: Some(response_bytes) }
    }

    pub fn url(&self) -> &Url {
        &self.url
    }

    pub fn finish(self) -> Vec<Vec<u8>> {
        self.finish_stats().0
    }

    pub fn finish_stats(self) -> (Vec<Vec<u8>>, u64) {
        if let Some(address) = self.stop_address {
            let mut stream = TcpStream::connect(address).expect("connect loopback shutdown");
            stream.write_all(b"GET /__stop HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").expect("stop loopback peer");
        }
        let requests = self.thread.join().expect("loopback peer completes");
        let response_bytes = self.response_bytes.map_or(0, |bytes| bytes.load(Ordering::Relaxed));
        (requests, u64::try_from(response_bytes).expect("loopback byte count fits u64"))
    }
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    read_request_inner(stream)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "request ended before framing completed"))
}

fn read_request_if_connected(stream: &mut TcpStream) -> Option<Vec<u8>> {
    read_request_inner(stream)
}

fn read_request_inner(stream: &mut TcpStream) -> Option<Vec<u8>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(header) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let length = String::from_utf8_lossy(&bytes[..header])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if bytes.len() >= header + 4 + length {
                return Some(bytes);
            }
        }
    }
}

fn write_response(stream: &mut TcpStream, status: u16, headers: Vec<(String, String)>, body: &[u8]) -> bool {
    let mut header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n", body.len());
    for (name, value) in headers {
        header.push_str(&format!("{name}: {value}\r\n"));
    }
    header.push_str("Connection: close\r\n\r\n");
    stream.write_all(header.as_bytes()).is_ok() && stream.write_all(body).is_ok()
}
