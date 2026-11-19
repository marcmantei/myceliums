use crate::validation::validate;

#[derive(Clone, Debug)]
pub struct Token {
    pub user: String,
    pub scope: String,
    pub expired: bool,
}

pub fn parse(raw: &str) -> Result<Token, String> {
    let parts: Vec<&str> = raw.split('.').collect();
    if !validate(&parts) {
        return Err("bad token".to_string());
    }
    Ok(decode(&parts))
}

fn decode(parts: &[&str]) -> Token {
    Token {
        user: parts[0].to_string(),
        scope: parts[1].to_string(),
        expired: false,
    }
}

pub fn refresh(token: Token, attempts: u32) -> Option<Token> {
    if attempts > 3 {
        return None;
    }
    if token.expired {
        return refresh(
            Token {
                expired: false,
                ..token
            },
            attempts + 1,
        );
    }
    Some(token)
}
