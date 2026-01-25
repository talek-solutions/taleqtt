const PONG_MESSAGE_IDENTIER: &str = "PONG";

pub struct PongReplicationMessage {
    replica_id: u16,
}

impl PongReplicationMessage {
    pub fn replica_id_from_message(message: &str) -> Result<u16, &'static str> {
        let mut parts = message.split_whitespace();

        match (parts.next(), parts.next()) {
            (Some(PONG_MESSAGE_IDENTIER), Some(id)) => {
                id.parse::<u16>().map_err(|_| "Invalid replica id")
            }
            (Some(_), _) => Err("Not a PONG message"),
            _ => Err("PONG message too short"),
        }
    }
}
