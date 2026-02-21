// Extracts session code, auth fragment, and encrypted video URL from bytegooty.com player HTML.

use anyhow::{Result, anyhow};
use regex::Regex;

use crate::exit_error;

/// Derive the "code" value by re-sorting scrambled characters according to
/// the index sequence embedded in the charset meta tag.
pub fn derive_code(html: &str) -> Result<String> {
    let csm_re = Regex::new(r#"(?i)<meta\s+charset=["']UTF-8["'][^>]+\bid=["']now_([^"']+)["']"#)?;
    let vpm_re = Regex::new(r#"(?i)<meta\s+name=["']viewport["'][^>]+\bid=["']now_([^"']+)["']"#)?;

    let csm = csm_re
        .captures(html)
        .ok_or_else(|| anyhow!("charset meta id not found in page HTML"))?;
    let vpm = vpm_re
        .captures(html)
        .ok_or_else(|| anyhow!("viewport meta id not found in page HTML"))?;

    let indices: Vec<usize> = csm[1].split('.').map(|s| s.parse().unwrap()).collect();
    let scrambled: Vec<char> = vpm[1].chars().collect();

    if indices.len() != scrambled.len() {
        exit_error!(
            "meta length mismatch: {} vs {}",
            indices.len(),
            scrambled.len()
        );
    }

    let mut pairs: Vec<(usize, char)> = indices.into_iter().zip(scrambled).collect();
    pairs.sort_by_key(|p| p.0);
    Ok(pairs.into_iter().map(|p| p.1).collect())
}

/// Build the authentication fragment from the theme-color and
/// msapplication-TileColor meta tags, ordered by the frag-order meta.
pub fn derive_fragment(html: &str) -> Result<String> {
    let content_re = Regex::new(r#"(?i)content=["']([^"']+)["']"#)?;
    let dv_re = Regex::new(r#"(?i)data-v=["']([^"']+)["']"#)?;

    let tc_re = Regex::new(r#"(?i)<meta\s+name=["']theme-color["'][^>]*>"#)?;
    let tc_tag = tc_re
        .captures(html)
        .ok_or_else(|| anyhow!("theme-color meta not found"))?[0]
        .to_string();

    let tc_content = content_re
        .captures(&tc_tag)
        .ok_or_else(|| anyhow!("theme-color: content attr missing"))?[1]
        .to_string();
    let tc_dv = dv_re
        .captures(&tc_tag)
        .ok_or_else(|| anyhow!("theme-color: data-v attr missing"))?[1]
        .to_string();

    let tl_re = Regex::new(r#"(?i)<meta\s+name=["']msapplication-TileColor["'][^>]*>"#)?;
    let tl_tag = tl_re
        .captures(html)
        .ok_or_else(|| anyhow!("msapplication-TileColor meta not found"))?[0]
        .to_string();

    let tl_content = content_re
        .captures(&tl_tag)
        .ok_or_else(|| anyhow!("TileColor: content attr missing"))?[1]
        .to_string();
    let tl_dv = dv_re
        .captures(&tl_tag)
        .ok_or_else(|| anyhow!("TileColor: data-v attr missing"))?[1]
        .to_string();

    let fom_re = Regex::new(r#"(?i)<meta\s+name=["']frag-order["'][^>]+content=["']([^"']+)["']"#)?;
    let fom = fom_re
        .captures(html)
        .ok_or_else(|| anyhow!("frag-order meta not found"))?;
    let frag_order: Vec<usize> = fom[1].split(',').map(|s| s.parse().unwrap()).collect();

    let vals = [tc_content, tc_dv, tl_content, tl_dv];
    let mut frag_arr = vec!["".to_string(); 4];
    for i in 0..4 {
        frag_arr[frag_order[i]] = vals[i].clone();
    }
    Ok(frag_arr.join(""))
}

/// Extract the base-64-encoded encrypted video URL from the inline JS config object.
pub fn extract_encrypted_url(html: &str) -> Result<String> {
    let re = Regex::new(r#"['"]url['"]\s*:\s*['"]([A-Za-z0-9+/=]{20,})['"]"#)?;
    let m = re
        .captures(html)
        .ok_or_else(|| anyhow!("config.url not found in page HTML"))?;
    Ok(m[1].to_string())
}
