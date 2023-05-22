// Copyright 2019-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

use std::sync::{Arc, Mutex};

use crate::{Config, Runtime, Window};
use url::Url;

/// IPC access configuration for a remote domain.
#[derive(Debug, Clone)]
pub struct RemoteDomainAccessScope {
  scheme: Option<String>,
  domain: String,
  windows: Vec<String>,
  plugins: Vec<String>,
}

impl RemoteDomainAccessScope {
  /// Creates a new access scope.
  pub fn new(domain: impl Into<String>) -> Self {
    Self {
      scheme: None,
      domain: domain.into(),
      windows: Vec::new(),
      plugins: Vec::new(),
    }
  }

  /// Sets the scheme of the URL to allow in this scope. By default, all schemes with the given domain are allowed.
  pub fn allow_on_scheme(mut self, scheme: impl Into<String>) -> Self {
    self.scheme.replace(scheme.into());
    self
  }

  /// Adds the given window label to the list of windows that uses this scope.
  pub fn add_window(mut self, window: impl Into<String>) -> Self {
    self.windows.push(window.into());
    self
  }

  /// Adds the given plugin to the allowed plugin list.
  pub fn add_plugin(mut self, plugin: impl Into<String>) -> Self {
    self.plugins.push(plugin.into());
    self
  }

  /// Adds the given list of plugins to the allowed plugin list.
  pub fn add_plugins<I, S>(mut self, plugins: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: Into<String>,
  {
    self.plugins.extend(plugins.into_iter().map(Into::into));
    self
  }

  /// The domain of the URLs that can access this scope.
  pub fn domain(&self) -> &str {
    &self.domain
  }

  /// The list of window labels that can access this scope.
  pub fn windows(&self) -> &Vec<String> {
    &self.windows
  }

  /// The list of plugins enabled by this scope.
  pub fn plugins(&self) -> &Vec<String> {
    &self.plugins
  }
}

pub(crate) struct RemoteAccessError {
  pub matches_window: bool,
  pub matches_domain: bool,
}

/// IPC scope.
#[derive(Clone)]
pub struct Scope {
  remote_access: Arc<Mutex<Vec<RemoteDomainAccessScope>>>,
}

impl Scope {
  pub(crate) fn new(config: &Config) -> Self {
    #[allow(unused_mut)]
    let mut remote_access: Vec<RemoteDomainAccessScope> = config
      .tauri
      .security
      .dangerous_remote_domain_ipc_access
      .clone()
      .into_iter()
      .map(|s| RemoteDomainAccessScope {
        scheme: s.scheme,
        domain: s.domain,
        windows: s.windows,
        plugins: s.plugins,
      })
      .collect();

    Self {
      remote_access: Arc::new(Mutex::new(remote_access)),
    }
  }

  /// Adds the given configuration for remote access.
  ///
  /// # Examples
  ///
  /// ```
  /// use tauri::{Manager, scope::ipc::RemoteDomainAccessScope};
  /// tauri::Builder::default()
  ///   .setup(|app| {
  ///     app.ipc_scope().configure_remote_access(
  ///       RemoteDomainAccessScope::new("tauri.app")
  ///         .add_window("main")
  ///         .add_plugins(["path", "event"])
  ///       );
  ///     Ok(())
  ///   });
  /// ```
  pub fn configure_remote_access(&self, access: RemoteDomainAccessScope) {
    self.remote_access.lock().unwrap().push(access);
  }

  pub(crate) fn remote_access_for<R: Runtime>(
    &self,
    window: &Window<R>,
    url: &Url,
  ) -> Result<RemoteDomainAccessScope, RemoteAccessError> {
    let mut scope = None;
    let mut found_scope_for_window = false;
    let mut found_scope_for_domain = false;
    let label = window.label().to_string();

    for s in &*self.remote_access.lock().unwrap() {
      #[allow(unused_mut)]
      let mut matches_window = s.windows.contains(&label);

      let matches_scheme = s
        .scheme
        .as_ref()
        .map(|scheme| scheme == url.scheme())
        .unwrap_or(true);

      let matches_domain =
        matches_scheme && url.domain().map(|d| d == s.domain).unwrap_or_default();
      found_scope_for_window = found_scope_for_window || matches_window;
      found_scope_for_domain = found_scope_for_domain || matches_domain;
      if matches_window && matches_domain && scope.is_none() {
        scope.replace(s.clone());
      }
    }

    if let Some(s) = scope {
      Ok(s)
    } else {
      Err(RemoteAccessError {
        matches_window: found_scope_for_window,
        matches_domain: found_scope_for_domain,
      })
    }
  }
}

#[cfg(test)]
mod tests {
  use serde::Serialize;

  use super::RemoteDomainAccessScope;
  use crate::{api::ipc::CallbackFn, test::MockRuntime, App, InvokePayload, Manager, Window};

  const PLUGIN_NAME: &str = "test";

  fn test_context(scopes: Vec<RemoteDomainAccessScope>) -> (App<MockRuntime>, Window<MockRuntime>) {
    let app = crate::test::mock_app();
    let window = app.get_window("main").unwrap();

    for scope in scopes {
      app.ipc_scope().configure_remote_access(scope);
    }

    (app, window)
  }

  fn assert_ipc_response<R: Serialize>(
    window: &Window<MockRuntime>,
    payload: InvokePayload,
    expected: Result<R, &str>,
  ) {
    let callback = payload.callback;
    let error = payload.error;
    window.clone().on_message(payload).unwrap();

    let mut num_tries = 0;
    let evaluated_script = loop {
      std::thread::sleep(std::time::Duration::from_millis(50));
      let evaluated_script = window.dispatcher().last_evaluated_script();
      if let Some(s) = evaluated_script {
        break s;
      }
      num_tries += 1;
      if num_tries == 20 {
        panic!("Response script not evaluated");
      }
    };
    let (expected_response, fn_name) = match expected {
      Ok(payload) => (serde_json::to_value(payload).unwrap(), callback),
      Err(payload) => (serde_json::to_value(payload).unwrap(), error),
    };
    let expected = format!(
      "window[\"_{}\"]({})",
      fn_name.0,
      crate::api::ipc::serialize_js(&expected_response).unwrap()
    );

    println!("Last evaluated script:");
    println!("{evaluated_script}");
    println!("Expected:");
    println!("{expected}");
    assert!(evaluated_script.contains(&expected));
  }

  fn path_is_absolute_payload() -> InvokePayload {
    let callback = CallbackFn(0);
    let error = CallbackFn(1);

    let mut payload = serde_json::Map::new();
    payload.insert(
      "path".into(),
      serde_json::Value::String(std::env::current_dir().unwrap().display().to_string()),
    );

    InvokePayload {
      cmd: "plugin:path|is_absolute".into(),
      callback,
      error,
      inner: serde_json::Value::Object(payload),
    }
  }

  fn plugin_test_payload() -> InvokePayload {
    let callback = CallbackFn(0);
    let error = CallbackFn(1);

    InvokePayload {
      cmd: format!("plugin:{PLUGIN_NAME}|doSomething"),
      callback,
      error,
      inner: Default::default(),
    }
  }

  #[test]
  fn scope_not_defined() {
    let (_app, window) = test_context(vec![RemoteDomainAccessScope::new("app.tauri.app")
      .add_window("other")
      .add_plugin("path")]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      path_is_absolute_payload(),
      Err(&crate::window::ipc_scope_not_found_error_message(
        "main",
        "https://tauri.app/",
      )),
    );
  }

  #[test]
  fn scope_not_defined_for_window() {
    let (_app, window) = test_context(vec![RemoteDomainAccessScope::new("tauri.app")
      .add_window("second")
      .add_plugin("path")]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      path_is_absolute_payload(),
      Err(&crate::window::ipc_scope_window_error_message("main")),
    );
  }

  #[test]
  fn scope_not_defined_for_url() {
    let (_app, window) = test_context(vec![RemoteDomainAccessScope::new("github.com")
      .add_window("main")
      .add_plugin("path")]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      path_is_absolute_payload(),
      Err(&crate::window::ipc_scope_domain_error_message(
        "https://tauri.app/",
      )),
    );
  }

  #[test]
  fn subdomain_is_not_allowed() {
    let (_app, mut window) = test_context(vec![
      RemoteDomainAccessScope::new("tauri.app")
        .add_window("main")
        .add_plugin("path"),
      RemoteDomainAccessScope::new("sub.tauri.app")
        .add_window("main")
        .add_plugin("path"),
    ]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response(&window, path_is_absolute_payload(), Ok(true));

    window.navigate("https://blog.tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      path_is_absolute_payload(),
      Err(&crate::window::ipc_scope_domain_error_message(
        "https://blog.tauri.app/",
      )),
    );

    window.navigate("https://sub.tauri.app".parse().unwrap());
    assert_ipc_response(&window, path_is_absolute_payload(), Ok(true));

    window.window.label = "test".into();
    window.navigate("https://dev.tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      path_is_absolute_payload(),
      Err(&crate::window::ipc_scope_not_found_error_message(
        "test",
        "https://dev.tauri.app/",
      )),
    );
  }

  #[test]
  fn subpath_is_allowed() {
    let (_app, window) = test_context(vec![RemoteDomainAccessScope::new("tauri.app")
      .add_window("main")
      .add_plugin("path")]);

    window.navigate("https://tauri.app/inner/path".parse().unwrap());
    assert_ipc_response(&window, path_is_absolute_payload(), Ok(true));
  }

  #[test]
  fn tauri_api_not_allowed() {
    let (_app, window) = test_context(vec![
      RemoteDomainAccessScope::new("tauri.app").add_window("main")
    ]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      path_is_absolute_payload(),
      Err(crate::window::IPC_SCOPE_DOES_NOT_ALLOW),
    );
  }

  #[test]
  fn plugin_allowed() {
    let (_app, window) = test_context(vec![RemoteDomainAccessScope::new("tauri.app")
      .add_window("main")
      .add_plugin(PLUGIN_NAME)]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      plugin_test_payload(),
      Err(&format!("plugin {PLUGIN_NAME} not found")),
    );
  }

  #[test]
  fn plugin_not_allowed() {
    let (_app, window) = test_context(vec![
      RemoteDomainAccessScope::new("tauri.app").add_window("main")
    ]);

    window.navigate("https://tauri.app".parse().unwrap());
    assert_ipc_response::<()>(
      &window,
      plugin_test_payload(),
      Err(crate::window::IPC_SCOPE_DOES_NOT_ALLOW),
    );
  }
}