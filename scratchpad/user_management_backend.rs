// ==================== 用户管理（users.php API）====================

const BWBROWSER_USERS_API_URL: &str = "https://www.yacm.xin/tk/users.php";

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CloudUserItem {
  pub id: i64,
  #[serde(default)]
  pub username: String,
  #[serde(default)]
  pub real_name: String,
  #[serde(default)]
  pub role: String,
  #[serde(default)]
  pub role_label: String,
  #[serde(default)]
  pub status: String,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_super_admin: Option<bool>,
  #[serde(default)]
  pub phone: Option<String>,
  #[serde(default)]
  pub email: Option<String>,
  #[serde(default)]
  pub created_at: Option<String>,
  #[serde(default)]
  pub last_login_at: Option<String>,
  #[serde(default)]
  pub permissions: Option<std::collections::BTreeMap<String, bool>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CloudUsersListResponse {
  pub success: bool,
  #[serde(default)]
  pub users: Option<Vec<CloudUserItem>>,
  #[serde(default, deserialize_with = "deserialize_int_or_bool")]
  pub is_super_admin: Option<bool>,
  #[serde(default)]
  pub permission_labels: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CloudRoleOption {
  pub value: String,
  pub label: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CloudRolesResponse {
  pub success: bool,
  #[serde(default)]
  pub roles: Option<Vec<CloudRoleOption>>,
}

impl BwbrowserAuthManager {
  /// 调用 users.php 的 JSON API
  async fn call_users_api(
    &self,
    action: &str,
    extra_params: &[(&str, &str)],
  ) -> Result<String, String> {
    let (username, password) = self
      .get_credentials()
      .ok_or_else(|| "未登录，请先登录云端账号".to_string())?;

    let mut form_parts: Vec<String> = vec![
      format!("action={}", urlencode(action)),
      format!("username={}", urlencode(&username)),
      format!("password={}", urlencode(&password)),
    ];
    for (k, v) in extra_params {
      form_parts.push(format!("{}={}", k, urlencode(v)));
    }
    let form_data = form_parts.join("&");

    log_bwbrowser("users", &format!("→ action={}", action));

    let resp = self
      .client
      .post(BWBROWSER_USERS_API_URL)
      .header("Content-Type", "application/x-www-form-urlencoded")
      .body(form_data)
      .send()
      .await
      .map_err(|e| {
        log_bwbrowser_error("users", &format!("网络请求失败: {}", e));
        format!("网络请求失败: {}", e)
      })?;

    let status = resp.status();
    let body = resp.text().await.map_err(|e| {
      log_bwbrowser_error("users", &format!("读取响应失败: {}", e));
      format!("读取响应失败: {}", e)
    })?;

    log_bwbrowser(
      "users",
      &format!(
        "← HTTP {}, {} bytes: {}",
        status,
        body.len(),
        body.chars().take(200).collect::<String>()
      ),
    );

    Ok(body)
  }

  /// 获取用户列表
  pub async fn list_cloud_users_management(&self) -> Result<CloudUsersListResponse, String> {
    let body = self.call_users_api("list", &[]).await?;
    let result: CloudUsersListResponse = parse_body("users_list", &body)?;
    Ok(result)
  }

  /// 获取角色列表
  pub async fn list_cloud_roles(&self) -> Result<Vec<CloudRoleOption>, String> {
    let body = self.call_users_api("roles", &[]).await?;
    let result: CloudRolesResponse = parse_body("users_roles", &body)?;
    Ok(result.roles.unwrap_or_default())
  }

  /// 添加用户
  pub async fn add_cloud_user(
    &self,
    username: &str,
    password: &str,
    real_name: &str,
    role: &str,
  ) -> Result<i64, String> {
    let body = self
      .call_users_api(
        "add",
        &[
          ("new_username", username),
          ("new_password", password),
          ("real_name", real_name),
          ("role", role),
        ],
      )
      .await?;
    let result: serde_json::Value = parse_body("users_add", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"]
        .as_str()
        .unwrap_or("添加失败")
        .to_string();
      return Err(msg);
    }
    let user_id = result["user_id"].as_i64().unwrap_or(0);
    Ok(user_id)
  }

  /// 更新用户信息
  pub async fn update_cloud_user(
    &self,
    user_id: i64,
    real_name: &str,
    role: &str,
    status: &str,
    password: Option<&str>,
  ) -> Result<(), String> {
    let mut params: Vec<(&str, String)> = vec![
      ("user_id", user_id.to_string()),
      ("real_name", real_name.to_string()),
      ("role", role.to_string()),
      ("status", status.to_string()),
    ];
    if let Some(pw) = password {
      if !pw.is_empty() {
        params.push(("password", pw.to_string()));
      }
    }
    let param_refs: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let body = self.call_users_api("update", &param_refs).await?;
    let result: serde_json::Value = parse_body("users_update", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"]
        .as_str()
        .unwrap_or("更新失败")
        .to_string();
      return Err(msg);
    }
    Ok(())
  }

  /// 切换用户权限
  pub async fn toggle_cloud_user_permission(
    &self,
    user_id: i64,
    permission: &str,
    value: bool,
  ) -> Result<(), String> {
    let body = self
      .call_users_api(
        "toggle_permission",
        &[
          ("user_id", &user_id.to_string()),
          ("permission", permission),
          ("value", if value { "1" } else { "0" }),
        ],
      )
      .await?;
    let result: serde_json::Value = parse_body("users_toggle_perm", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"]
        .as_str()
        .unwrap_or("操作失败")
        .to_string();
      return Err(msg);
    }
    Ok(())
  }

  /// 切换用户状态（启用/禁用）
  pub async fn toggle_cloud_user_status(&self, user_id: i64, status: &str) -> Result<(), String> {
    let body = self
      .call_users_api(
        "toggle_status",
        &[("user_id", &user_id.to_string()), ("status", status)],
      )
      .await?;
    let result: serde_json::Value = parse_body("users_toggle_status", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"]
        .as_str()
        .unwrap_or("操作失败")
        .to_string();
      return Err(msg);
    }
    Ok(())
  }

  /// 删除用户
  pub async fn delete_cloud_user(&self, user_id: i64) -> Result<(), String> {
    let body = self
      .call_users_api("delete", &[("user_id", &user_id.to_string())])
      .await?;
    let result: serde_json::Value = parse_body("users_delete", &body)?;
    if !result["success"].as_bool().unwrap_or(false) {
      let msg = result["message"]
        .as_str()
        .unwrap_or("删除失败")
        .to_string();
      return Err(msg);
    }
    Ok(())
  }
}

// ========== Tauri Commands - 用户管理 ==========

#[tauri::command]
pub async fn bwbrowser_list_management_users() -> Result<CloudUsersListResponse, String> {
  let result = BWBROWSER_AUTH.list_cloud_users_management().await?;
  Ok(result)
}

#[tauri::command]
pub async fn bwbrowser_list_management_roles() -> Result<Vec<CloudRoleOption>, String> {
  let roles = BWBROWSER_AUTH.list_cloud_roles().await?;
  Ok(roles)
}

#[tauri::command]
pub async fn bwbrowser_add_management_user(
  username: String,
  password: String,
  real_name: String,
  role: String,
) -> Result<i64, String> {
  let user_id = BWBROWSER_AUTH
    .add_cloud_user(&username, &password, &real_name, &role)
    .await?;
  Ok(user_id)
}

#[tauri::command]
pub async fn bwbrowser_update_management_user(
  user_id: i64,
  real_name: String,
  role: String,
  status: String,
  password: Option<String>,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .update_cloud_user(
      user_id,
      &real_name,
      &role,
      &status,
      password.as_deref(),
    )
    .await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_toggle_management_user_permission(
  user_id: i64,
  permission: String,
  value: bool,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .toggle_cloud_user_permission(user_id, &permission, value)
    .await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_toggle_management_user_status(
  user_id: i64,
  status: String,
) -> Result<(), String> {
  BWBROWSER_AUTH
    .toggle_cloud_user_status(user_id, &status)
    .await?;
  Ok(())
}

#[tauri::command]
pub async fn bwbrowser_delete_management_user(user_id: i64) -> Result<(), String> {
  BWBROWSER_AUTH.delete_cloud_user(user_id).await?;
  Ok(())
}
