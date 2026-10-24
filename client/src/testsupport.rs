//! A tiny scripted HTTP server for the client tests.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use tiny_http::{Header, Server};
use url::Url;

#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub struct TestResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl TestResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        TestResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

pub struct TestServer {
    address: SocketAddr,
    server: Arc<Server>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl TestServer {
    pub fn spawn<F>(handler: F) -> Self
    where
        F: Fn(&RecordedRequest) -> TestResponse + Send + 'static,
    {
        let server = Arc::new(Server::http("127.0.0.1:0").expect("bind test server"));
        let address = server
            .server_addr()
            .to_ip()
            .expect("test server must be TCP");
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));

        let handle = {
            let server = Arc::clone(&server);
            let stop = Arc::clone(&stop);
            let requests = Arc::clone(&requests);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match server.recv_timeout(Duration::from_millis(50)) {
                        Ok(Some(request)) => {
                            let recorded = RecordedRequest {
                                path: request.url().to_string(),
                                headers: request
                                    .headers()
                                    .iter()
                                    .map(|header| {
                                        (
                                            header.field.as_str().to_string(),
                                            header.value.as_str().to_string(),
                                        )
                                    })
                                    .collect(),
                            };
                            requests.lock().unwrap().push(recorded.clone());
                            send(request, handler(&recorded));
                        }
                        Ok(None) => {}
                        Err(_) => break,
                    }
                }
            })
        };

        TestServer {
            address,
            server,
            stop,
            handle: Some(handle),
            requests,
        }
    }

    pub fn base_url(&self) -> Url {
        Url::parse(&format!("http://{}/", self.address)).unwrap()
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn send(request: tiny_http::Request, response: TestResponse) {
    let mut http_response =
        tiny_http::Response::from_data(response.body).with_status_code(response.status);
    for (name, value) in response.headers {
        if let Ok(header) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            http_response.add_header(header);
        }
    }
    let _ = request.respond(http_response);
}

/// Encode an RGB PNG for tests.
pub fn rgb_png(width: u32, height: u32, pixels: &[[u8; 3]]) -> Vec<u8> {
    assert_eq!(pixels.len(), (width * height) as usize);
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&pixels.concat())
            .expect("write test png");
        writer.finish().unwrap();
    }
    bytes
}
