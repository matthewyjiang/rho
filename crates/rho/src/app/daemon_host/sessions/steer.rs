//! Steering a session's running turn: staging input for its next model
//! request, and withdrawing a steer before it applies.
//!
//! A steer answers with the id the run accepted it under, and the session's
//! live mapper keeps its content until the run applies it, which shows as a
//! user message carrying that id (see
//! [`crate::app::session_events::LiveEvents::steered`]). Neither request
//! loads a session: one that is not loaded has no turn to steer.

use rho_protocol::{
    ErrorCode, RpcError, SessionSteerParams, SessionSteerResponse, SessionUnsteerParams,
    SessionUnsteerResponse, UnsteerOutcome,
};
use rho_sdk::{SteeringId, SteeringRetraction, UserInput};

use super::Sessions;
use crate::app::{session_events::steer_to_wire, session_runtime::SteerError};

impl Sessions {
    /// Stages `text` for the running turn's next model request, and answers
    /// once the run accepted it, with the id it applies under.
    pub(in crate::app::daemon_host) async fn steer(
        &self,
        params: SessionSteerParams,
    ) -> Result<SessionSteerResponse, RpcError> {
        let Some(session) = self.loaded_session(&params.session).await? else {
            return Err(not_steerable());
        };
        let input = UserInput::text(params.text);
        let receipt = session
            .handle
            .steer(input.clone())
            .map_err(|error| refused(error, "steer session"))?;
        let id = receipt
            .await
            .map_err(|error| refused(SteerError::from_sdk(error), "steer session"))?;
        session.live().steered(id.clone(), &input);
        Ok(SessionSteerResponse {
            id: steer_to_wire(&id),
        })
    }

    /// Withdraws a steer the running turn has not applied yet, and answers
    /// with what became of it.
    pub(in crate::app::daemon_host) async fn unsteer(
        &self,
        params: SessionUnsteerParams,
    ) -> Result<SessionUnsteerResponse, RpcError> {
        let Some(session) = self.loaded_session(&params.session).await? else {
            return Err(not_steerable());
        };
        let id = SteeringId::from_string(params.id.as_str())
            .map_err(|_| RpcError::new(ErrorCode::InvalidParams, "not a steer id"))?;
        let retraction = session
            .handle
            .retract(id.clone())
            .map_err(|error| refused(error, "withdraw the steer"))?;
        let retraction = retraction
            .await
            .map_err(|error| refused(SteerError::from_sdk(error), "withdraw the steer"))?;
        let outcome = match retraction {
            SteeringRetraction::Retracted => {
                session.live().withdrawn(&id);
                UnsteerOutcome::Withdrawn
            }
            SteeringRetraction::Delivered => UnsteerOutcome::Delivered,
            SteeringRetraction::AlreadyApplied => UnsteerOutcome::Applied,
            SteeringRetraction::NotFound => UnsteerOutcome::NotFound,
        };
        Ok(SessionUnsteerResponse { outcome })
    }
}

/// The error a steer or unsteer answers while no model request is taking
/// input.
fn not_steerable() -> RpcError {
    RpcError::new(ErrorCode::NotSteerable, "no model request is taking input")
}

/// The error a steer or unsteer the session's turn refused answers:
/// `NotSteerable` once no model request takes input, else `SteerRefused`
/// saying it could not `action`, and why.
fn refused(error: SteerError, action: &str) -> RpcError {
    match error {
        SteerError::NotRunning => not_steerable(),
        SteerError::Refused(error) => RpcError::new(
            ErrorCode::SteerRefused,
            format!("could not {action}: {error}"),
        ),
    }
}
