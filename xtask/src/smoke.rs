//! `cargo xtask mcp-smoke --url URL`: the release job's check that a built
//! loctel answers MCP.
//!
//! It connects the way Claude Code does (rmcp's client over Streamable HTTP),
//! asks which tools there are, and requires `recent_failures`, the tool the
//! product exists for. Anything else is a failure: no server, a server that is
//! not MCP, or one without that tool.

use std::time::Duration;

use rmcp::ServiceExt;
use rmcp::model::ClientConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

/// The tool every loctel must list.
const REQUIRED_TOOL: &str = "recent_failures";

/// How long the whole check may take, so that a hung server fails the release
/// job in seconds and not after the runner's own timeout.
const TIMEOUT: Duration = Duration::from_secs(30);

/// Connects to the MCP server at `url` (`http://127.0.0.1:4319/mcp`), lists its
/// tools and checks that `recent_failures` is among them. On success it prints
/// one line saying so.
pub fn mcp_smoke(url: &str) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the async runtime: {error}"))?;
    let names = runtime.block_on(async {
        tokio::time::timeout(TIMEOUT, list_tool_names(url))
            .await
            .map_err(|_| format!("the MCP server at {url} did not answer within {TIMEOUT:?}"))?
    })?;
    check_tools(&names)?;
    println!(
        "mcp-smoke: {url} lists {REQUIRED_TOOL} ({} tools in all)",
        names.len()
    );
    Ok(())
}

/// The names of the tools the server at `url` lists.
async fn list_tool_names(url: &str) -> Result<Vec<String>, String> {
    // No proxy: loctel is on this machine, and a proxy variable in the
    // environment must not carry the request away.
    let http = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| format!("cannot build the HTTP client: {error}"))?;
    let transport = StreamableHttpClientTransport::with_client(
        http,
        StreamableHttpClientTransportConfig::with_uri(url.to_owned()),
    );
    let connection = ClientConfig::default()
        .serve(transport)
        .await
        .map_err(|error| format!("cannot connect to MCP at {url}: {error}"))?;
    let tools = connection
        .list_all_tools()
        .await
        .map_err(|error| format!("cannot list the tools of {url}: {error}"))?;
    // Ending the session politely is a courtesy; the answer is already in.
    let _ = connection.cancel().await;
    Ok(tools
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect())
}

/// `Ok` when `names` includes `recent_failures`; otherwise the error says what
/// the server did list.
fn check_tools(names: &[String]) -> Result<(), String> {
    if names.iter().any(|name| name == REQUIRED_TOOL) {
        Ok(())
    } else if names.is_empty() {
        Err(format!(
            "the MCP server lists no tools, and `{REQUIRED_TOOL}` is required"
        ))
    } else {
        Err(format!(
            "the MCP server does not list `{REQUIRED_TOOL}`; it lists: {}",
            names.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use loctel_engine::{Engine, EngineConfig, Settings};

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_required_tool_among_others_passes() {
        assert_eq!(
            check_tools(&names(&["get_status", "recent_failures", "get_trace"])),
            Ok(())
        );
    }

    #[test]
    fn a_missing_required_tool_fails_and_names_what_is_there() {
        let error = check_tools(&names(&["get_status", "get_trace"])).unwrap_err();

        assert!(error.contains("recent_failures"), "{error}");
        assert!(error.contains("get_status, get_trace"), "{error}");
    }

    #[test]
    fn no_tools_at_all_fails() {
        assert!(check_tools(&[]).unwrap_err().contains("no tools"));
    }

    #[test]
    fn passes_against_a_real_engine() {
        // The same engine `loctel --headless` runs, on free ports.
        let engine = Engine::start(EngineConfig {
            settings: Settings {
                otlp_grpc_port: 0,
                otlp_http_port: 0,
                mcp_port: 0,
                ..Settings::default()
            },
            ..EngineConfig::default()
        })
        .expect("the engine starts");
        let url = engine.mcp_url().expect("the MCP server is listening");

        let outcome = mcp_smoke(&url);
        engine.shutdown();

        assert_eq!(outcome, Ok(()));
    }

    #[test]
    fn fails_when_nothing_is_listening() {
        // A port that was free a moment ago and now has nobody on it.
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("a free port")
            .port();

        let error = mcp_smoke(&format!("http://127.0.0.1:{port}/mcp")).unwrap_err();

        assert!(error.contains("cannot connect"), "{error}");
    }
}
