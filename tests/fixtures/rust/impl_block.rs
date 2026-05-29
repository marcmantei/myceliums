struct Server {
    port: u16,
    host: String,
    running: bool,
}

impl Server {
    fn new(port: u16, host: String) -> Self {
        Server {
            port,
            host,
            running: false,
        }
    }

    fn start(&mut self) -> Result<(), String> {
        self.running = true;
        Ok(())
    }

    fn stop(&mut self) {
        self.running = false;
    }

    fn is_running(&self) -> bool {
        self.running
    }
}

struct Client {
    server_url: String,
}

impl Client {
    fn connect(url: &str) -> Self {
        Client {
            server_url: url.to_string(),
        }
    }

    fn send(&self, data: &[u8]) -> Result<(), String> {
        Ok(())
    }
}
