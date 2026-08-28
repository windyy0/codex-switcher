//! Isolated disk/process/transport fixtures. No real credentials, processes,
//! environment variables, or authorization servers are touched by these tests.

use std::{cell::RefCell, collections::VecDeque, path::PathBuf};

use crate::types::{AccountsStore, StoredAccount};
use anyhow::Result;
use base64::Engine;
use serde_json::Value;

struct State {
    root: PathBuf,
    running: bool,
    process_error: bool,
    start_during_refresh: bool,
    responses: VecDeque<Result<Value, String>>,
    requests: Vec<String>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub(crate) struct AuthTestEnv {
    root: PathBuf,
}

impl AuthTestEnv {
    /// Use on the default current-thread Tokio test runtime. File-lock workers
    /// receive their resolved path explicitly instead of reading thread state.
    pub(crate) fn new() -> Self {
        let root = std::env::temp_dir().join(format!("codex-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("codex")).unwrap();
        STATE.with(|state| {
            assert!(state.borrow().is_none(), "nested auth fixture");
            *state.borrow_mut() = Some(State {
                root: root.clone(),
                running: false,
                process_error: false,
                start_during_refresh: false,
                responses: VecDeque::new(),
                requests: Vec::new(),
            });
        });
        Self { root }
    }

    pub(crate) fn running(&self, running: bool) {
        STATE.with(|state| state.borrow_mut().as_mut().unwrap().running = running);
    }

    pub(crate) fn process_error(&self) {
        STATE.with(|state| state.borrow_mut().as_mut().unwrap().process_error = true);
    }

    pub(crate) fn start_codex_during_refresh(&self) {
        STATE.with(|state| state.borrow_mut().as_mut().unwrap().start_during_refresh = true);
    }

    pub(crate) fn respond(&self, response: Value) {
        STATE.with(|state| {
            state
                .borrow_mut()
                .as_mut()
                .unwrap()
                .responses
                .push_back(Ok(response))
        });
    }

    pub(crate) fn fail_refresh(&self) {
        STATE.with(|state| {
            state
                .borrow_mut()
                .as_mut()
                .unwrap()
                .responses
                .push_back(Err("simulated transport failure".into()))
        });
    }

    pub(crate) fn requests(&self) -> Vec<String> {
        STATE.with(|state| state.borrow().as_ref().unwrap().requests.clone())
    }
}

impl Drop for AuthTestEnv {
    fn drop(&mut self) {
        STATE.with(|state| *state.borrow_mut() = None);
        // This UUID directory was created by this fixture, never supplied by a user.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn is_active() -> bool {
    STATE.with(|state| state.borrow().is_some())
}

pub(crate) fn config_dir() -> Option<PathBuf> {
    STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| state.root.join("switcher"))
    })
}

pub(crate) fn codex_home() -> Option<PathBuf> {
    STATE.with(|state| {
        state
            .borrow()
            .as_ref()
            .map(|state| state.root.join("codex"))
    })
}

pub(crate) fn codex_running() -> Option<Result<bool>> {
    STATE.with(|state| {
        state.borrow().as_ref().map(|state| {
            if state.process_error {
                anyhow::bail!("simulated process inspection failure");
            }
            Ok(state.running)
        })
    })
}

pub(crate) async fn refresh_response(token: &str) -> Result<Value> {
    let response = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .expect("mock transport requires an isolated fixture");
        state.requests.push(token.to_string());
        if state.start_during_refresh {
            state.running = true;
        }
        state
            .responses
            .pop_front()
            .expect("unexpected refresh request (real network is disabled)")
    });
    // Exercise overlapping callers while the credential lock remains held.
    tokio::task::yield_now().await;
    response.map_err(anyhow::Error::msg)
}

pub(crate) fn jwt(identity: Option<&str>, exp: Option<i64>) -> String {
    let mut payload = serde_json::json!({"email": "same@example.com"});
    if let Some(identity) = identity {
        payload["https://api.openai.com/auth"] =
            serde_json::json!({"chatgpt_account_id": identity});
    }
    if let Some(exp) = exp {
        payload["exp"] = exp.into();
    }
    format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).unwrap())
    )
}

pub(crate) fn account(identity: &str) -> StoredAccount {
    let token = jwt(Some(identity), Some(chrono::Utc::now().timestamp() + 3600));
    StoredAccount::new_chatgpt(
        identity.into(),
        Some("same@example.com".into()),
        None,
        None,
        token.clone(),
        token,
        format!("{identity}-refresh"),
        Some(identity.into()),
    )
}

pub(crate) fn seed_accounts(accounts: Vec<StoredAccount>, active: Option<&str>) {
    assert!(is_active(), "disk tests must use an isolated fixture");
    let store = AccountsStore {
        accounts,
        active_account_id: active.map(str::to_owned),
        active_account_home: active.map(|_| super::get_codex_home_identity().unwrap()),
        ..AccountsStore::default()
    };
    super::save_accounts(&store).unwrap();
    if let Some(active) = active {
        super::sync_account_auth_file(
            store
                .accounts
                .iter()
                .find(|account| account.id == active)
                .unwrap(),
        )
        .unwrap();
    }
}
