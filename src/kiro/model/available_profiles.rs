//! 可用 Profile 查询数据模型
//!
//! 对应上游 `ListAvailableProfiles`（AWS JSON 1.0，target
//! `AmazonCodeWhispererService.ListAvailableProfiles`）的响应类型。

#![allow(dead_code)]

use serde::Deserialize;

/// `ListAvailableProfiles` 响应。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ListAvailableProfilesResponse {
    /// 该凭据可用的 profile 列表。
    #[serde(default)]
    pub profiles: Vec<AvailableProfile>,

    /// 分页 token。
    #[serde(default)]
    #[allow(dead_code)]
    pub next_token: Option<String>,
}

/// 单个可用 profile。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AvailableProfile {
    /// 真实可用的 profileArn。
    #[serde(default)]
    pub arn: Option<String>,

    /// Profile 名称。
    #[serde(default)]
    #[allow(dead_code)]
    pub profile_name: Option<String>,
}

impl ListAvailableProfilesResponse {
    /// 返回第一个非空的 profileArn。
    pub fn first_arn(&self) -> Option<&str> {
        self.profiles
            .iter()
            .filter_map(|profile| profile.arn.as_deref())
            .find(|arn| !arn.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_profiles_and_first_arn() {
        let response: ListAvailableProfilesResponse = serde_json::from_str(
            r#"{
                "profiles": [{
                    "arn": "arn:aws:codewhisperer:us-east-1:610548660232:profile/VNECVYCYYAWN",
                    "profileName": "KiroProfile-us-east-1",
                    "identityDetails": {"ssoIdentityDetails": {"ssoRegion": "us-east-1"}}
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(
            response.first_arn(),
            Some("arn:aws:codewhisperer:us-east-1:610548660232:profile/VNECVYCYYAWN")
        );
    }

    #[test]
    fn test_first_arn_none_when_empty() {
        let response: ListAvailableProfilesResponse =
            serde_json::from_str(r#"{"profiles":[]}"#).unwrap();
        assert_eq!(response.first_arn(), None);
    }

    #[test]
    fn test_first_arn_skips_blank() {
        let response: ListAvailableProfilesResponse =
            serde_json::from_str(r#"{"profiles":[{"arn":""},{"arn":"arn:real"}]}"#).unwrap();
        assert_eq!(response.first_arn(), Some("arn:real"));
    }
}
