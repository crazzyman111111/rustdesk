//! 后台策略下发（MGS Policy）
//!
//! 客户端启动后由本模块定期向控制台拉取「策略下发」结果并就地应用：
//!   - 显示类参数（view_style / scroll_style / image_quality ...）
//!       写 UserDefaultConfig（RustDesk_default.toml 的 [options]），
//!       即「设置 → 显示」读取的那套存储；
//!   - 其余参数写 Config::set_option（RustDesk2.toml 的 [options]）。
//!
//! 只有服务端版本号变大时才写配置，避免无谓 IO。
//! 与「设置页管理员密码」无关：那是 Flutter 侧的界面闸门，本模块走的是内核配置层。

use hbb_common::{
    anyhow::anyhow,
    config::{Config, UserDefaultConfig},
    log,
    ResultType,
};
use serde_json::{json, Map, Value};
use std::sync::Once;
use std::time::Duration;

/// 记录已应用的策略版本（写进 RustDesk2.toml 的 [options]，随配置持久化）
const VER_KEY: &str = "mgs-policy-version";

static START: Once = Once::new();

/// 显示类键：必须写 UserDefaultConfig，写进 Config 是不生效的（两套独立存储）
fn is_display_key(k: &str) -> bool {
    matches!(
        k,
        "view_style"
            | "scroll_style"
            | "image_quality"
            | "codec_preference"
            | "custom_image_quality"
            | "custom-fps"
            | "edge-scroll-edge-thickness"
            | "trackpad_speed"
            | "enable-file-copy-paste"
    )
}

fn api_base() -> String {
    crate::ui_interface::get_api_server()
        .trim_end_matches('/')
        .to_owned()
}

fn tick() -> ResultType<()> {
    let id = Config::get_id();
    if id.is_empty() {
        return Ok(());
    }
    let base = api_base();
    if base.is_empty() {
        return Ok(());
    }
    let host = crate::common::hostname();
    let url = format!("{}/api/mgs/policy?id={}&host={}", base, id, host);
    let cli = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| anyhow!("{e}"))?;
    let resp = cli.get(&url).send().map_err(|e| anyhow!("{e}"))?;
    let v: Value = resp.json().map_err(|e| anyhow!("{e}"))?;
    if v.get("code").and_then(|x| x.as_i64()) != Some(0) {
        return Ok(());
    }
    let data = &v["data"];
    let ver = data["version"].as_i64().unwrap_or(0);
    if ver <= 0 {
        return Ok(());
    }
    let cur: i64 = Config::get_option(VER_KEY).trim().parse().unwrap_or(0);
    if ver <= cur {
        return Ok(());
    }
    let mut applied = Map::new();
    if let Some(opts) = data["options"].as_object() {
        for (k, val) in opts {
            let Some(sv) = val.as_str() else { continue };
            if is_display_key(k) {
                let mut udc = UserDefaultConfig::load();
                if udc.get(k) != sv {
                    udc.set(k.clone(), sv.to_owned());
                }
            } else if Config::get_option(k) != sv {
                Config::set_option(k.clone(), sv.to_owned());
            }
            applied.insert(k.clone(), Value::String(sv.to_owned()));
        }
    }
    Config::set_option(VER_KEY.to_owned(), ver.to_string());
    let body = json!({
        "id": id,
        "host": host,
        "version": ver,
        "applied": applied,
    });
    let _ = cli
        .post(format!("{}/api/mgs/policy/ack", base))
        .json(&body)
        .send();
    log::info!("MGS policy v{} applied: {} key(s)", ver, applied.len());
    Ok(())
}

/// 幂等启动：只在进程内起一次后台线程
pub fn start() {
    START.call_once(|| {
        std::thread::spawn(|| {
            // 等配置/网络就绪，避免启动瞬间打扰登录流程
            std::thread::sleep(Duration::from_secs(20));
            loop {
                if let Err(e) = tick() {
                    log::debug!("MGS policy: {e}");
                }
                std::thread::sleep(Duration::from_secs(180));
            }
        });
    });
}
