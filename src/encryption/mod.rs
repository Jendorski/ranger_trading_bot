use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
//use log::info;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

//bitget uses Hmac with base64 encoding
pub fn bitget_sign(
    secret: &str,
    timestamp: &str,
    method: &str,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> String {
    let mut prehash = String::new();
    prehash.push_str(timestamp);
    prehash.push_str(&method.to_uppercase());
    prehash.push_str(path);

    if let Some(q) = query {
        if !q.is_empty() {
            prehash.push('?');
            prehash.push_str(q);
        }
    }

    if let Some(b) = body {
        prehash.push_str(b);
    }

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(prehash.as_bytes());
    general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

// pub fn binance_sign() {
//     //todo
//     info!("binance not available yet!");
// }

// pub fn hyperliquid_sign() {
//     //todo
//     info!("hyperliquid not available yet!");
// }
