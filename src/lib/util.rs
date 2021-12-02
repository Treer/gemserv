use sha2::Digest;
use url::form_urlencoded;

pub fn url_decode(url: &[u8]) -> String {
    let decoded: String = form_urlencoded::parse(url)
        .map(|(key, val)| [key, val].concat())
        .collect();
    return decoded;
}
pub fn fingerhex(x509: &[u8]) -> String {
    let mut finger = sha2::Sha256::new();
    finger.update(&x509);
    let finger = finger.finalize();
    let mut hex: String = String::from("SHA256:");
    for f in finger {
        hex.push_str(&format!("{:02X}", f));
    }
    hex
}
