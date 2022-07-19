use rand::Rng;
use std::{error, str};
pub type Error = Box<dyn error::Error>;

#[derive(Debug)]
pub struct SdpFields {
  pub ice_ufrag: String,
  pub ice_passwd: String,
  pub mid: String,
}

pub fn parse_sdp_fields(body: &str) -> Result<SdpFields, Error> {
  let reader = body.as_bytes();
  const MAX_SDP_LINE: usize = 512;

  fn after_prefix<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    if s.starts_with(prefix) {
      Some(&s[prefix.len()..])
    } else {
      None
    }
  }
  let mut line_buf = Vec::new();
  line_buf.reserve(MAX_SDP_LINE);
  let mut found_ice_ufrag = None;
  let mut found_ice_passwd = None;
  let mut found_mid = None;
  for &c in reader.as_ref() {
    if c == b'\r' || c == b'\n' {
      if !line_buf.is_empty() {
        if let Some(ice_ufrag) = after_prefix(&line_buf, b"a=ice-ufrag:") {
          found_ice_ufrag = Some(String::from_utf8(ice_ufrag.to_vec())?);
        }
        if let Some(ice_passwd) = after_prefix(&line_buf, b"a=ice-pwd:") {
          found_ice_passwd = Some(String::from_utf8(ice_passwd.to_vec())?);
        }
        if let Some(mid) = after_prefix(&line_buf, b"a=mid:") {
          found_mid = Some(String::from_utf8(mid.to_vec())?);
        }
        line_buf.clear();
      }
    } else {
      if line_buf.len() < MAX_SDP_LINE {
        line_buf.push(c);
      }
    }
  }
  match (found_ice_ufrag, found_ice_passwd, found_mid) {
    (Some(ice_ufrag), Some(ice_passwd), Some(mid)) => Ok(SdpFields {
      ice_ufrag,
      ice_passwd,
      mid,
    }),
    _ => Err("not all SDP fields provided".into()),
  }
}

pub fn gen_sdp_response<R: Rng>(
  rng: &mut R,
  cert_fingerprint: &str,
  server_ip: &str,
  server_is_ipv6: bool,
  server_port: u16,
  ufrag: &str,
  pass: &str,
  remote_mid: &str,
) -> String {
  format!(
    "{{\"answer\":{{\"sdp\":\"v=0\\r\\n\
         o=FTL {rand1} 1 IN {ipv} {ip}\\r\\n\
         s=-\\r\\n\
         c=IN {ipv} {ip}\\r\\n\
         t=0 0\\r\\n\
         a=ice-lite\\r\\n\
         a=ice-ufrag:{ufrag}\\r\\n\
         a=ice-pwd:{pass}\\r\\n\
         m=application {port} UDP/DTLS/SCTP webrtc-datachannel\\r\\n\
         a=max-message-size:1160\\r\\n\
         a=fingerprint:sha-256 {fingerprint}\\r\\n\
         a=ice-options:trickle\\r\\n\
         a=setup:passive\\r\\n\
         a=mid:{mid}\\r\\n\
		     a=sctpmap:{port} webrtc-datachannel 8000\\r\\n\
         a=max-message-size:1160\\r\\n\
         a=sendrecv\\r\\n\
         a=sctp-port:{port}\\r\\n\",\
         \"type\":\"answer\"}},\"candidate\":{{\"sdpMLineIndex\":0,\
         \"sdpMid\":\"{mid}\",\"candidate\":\"candidate:1 1 UDP {rand2} {ip} {port} \
         typ host\"}}}}",
    rand1 = rng.gen::<u32>(),
    rand2 = rng.gen::<u32>(),
    fingerprint = cert_fingerprint,
    ip = server_ip,
    port = server_port,
    ufrag = ufrag,
    pass = pass,
    mid = remote_mid,
    ipv = if server_is_ipv6 { "IP6" } else { "IP4" },
  )
}
