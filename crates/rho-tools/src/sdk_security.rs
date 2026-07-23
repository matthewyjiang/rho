use rho_sdk::{
    tool::{ToolContext, ToolError},
    CapabilityRequest,
};

pub async fn authorize_request(
    context: &ToolContext,
    request: CapabilityRequest,
) -> Result<(), ToolError> {
    context
        .authorize(request)
        .await
        .map(|_| ())
        .map_err(|error| {
            if error.kind() == rho_sdk::AuthorizationDenialKind::Cancelled {
                ToolError::cancelled()
            } else {
                ToolError::policy_denied(&error)
            }
        })
}

#[cfg(test)]
#[path = "sdk_security_tests.rs"]
mod tests;
