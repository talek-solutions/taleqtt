use std::time::SystemTime;
const PING_MESSAGE_IDENTIER: &str = "PING";

pub struct PingReplicationMessage {
    master_id: u16,
}

impl PingReplicationMessage {
    pub fn new(master_id: u16) -> String {
        format!(
            "{} {} {}",
            PING_MESSAGE_IDENTIER,
            master_id,
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        )
    }
}
