use anyhow::Result;

use super::ClaimVerifyResult;

impl ClaimVerifyResult {
    /// Serialize this claim record for persistence or transport.
    pub fn to_json_string(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }
}
