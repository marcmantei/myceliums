pub type Rule = fn(&str) -> bool;

pub fn validate(parts: &[&str]) -> bool {
    parts.len() == 2 && parts.iter().all(|part| !part.is_empty())
}

pub fn is_email(value: &str) -> bool {
    value.contains('@')
}

pub struct Validator {
    rules: Vec<Rule>,
}

impl Validator {
    pub fn new(rules: Vec<Rule>) -> Self {
        Validator { rules }
    }

    pub fn validate(&self, value: &str) -> bool {
        self.rules.iter().all(|rule| self.check(*rule, value))
    }

    fn check(&self, rule: Rule, value: &str) -> bool {
        rule(value)
    }
}
