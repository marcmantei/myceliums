trait Handler {
    fn handle(&self, input: &str) -> String;
    fn name(&self) -> &str;
}

trait Validator {
    fn validate(&self) -> bool;
}

struct EchoHandler {
    prefix: String,
}

impl Handler for EchoHandler {
    fn handle(&self, input: &str) -> String {
        format!("{}: {}", self.prefix, input)
    }

    fn name(&self) -> &str {
        "echo"
    }
}

impl Validator for EchoHandler {
    fn validate(&self) -> bool {
        !self.prefix.is_empty()
    }
}
