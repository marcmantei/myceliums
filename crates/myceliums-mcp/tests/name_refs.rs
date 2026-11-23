//! References by qualified name (#179) over a real MCP session.
//!
//! A project with a Spring auto-configuration (a class named in an
//! `*.imports` file, an XML bean, `@ConditionalOnClass(name = ...)`) and a
//! Django-like flow (`runserver` → `get_internal_wsgi_application`, which
//! loads `WSGIHandler` with `import_string("...")`; a settings module naming
//! `ExceptionReporter`). `find_callers`, `find_callees`, `impact_of_change`
//! and `trace_flow` must list these places, which no CALLS edge shows.
//!
//! This binary owns its data directory, so it may analyze.

use myceliums_mcp::{AnalyzeParams, MyceliumsMcp, Toolsets};
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use serde_json::{json, Value};

type Client = RunningService<RoleClient, ()>;

const FILES: &[(&str, &str)] = &[
    (
        "src/main/java/org/acme/client/AcmeClient.java",
        "package org.acme.client;\n\npublic class AcmeClient {\n    public void send() {}\n}\n",
    ),
    (
        "src/main/java/org/acme/autoconfigure/AcmeAutoConfiguration.java",
        "package org.acme.autoconfigure;\n\n@ConditionalOnClass(name = \"org.acme.client.AcmeClient\")\npublic class AcmeAutoConfiguration {\n    public Object client() throws Exception {\n        return Class.forName(\"org.acme.client.AcmeClient\");\n    }\n}\n",
    ),
    (
        "src/main/resources/META-INF/spring/org.springframework.boot.autoconfigure.AutoConfiguration.imports",
        "org.acme.autoconfigure.AcmeAutoConfiguration\n",
    ),
    (
        "src/main/resources/beans.xml",
        "<beans>\n  <bean id=\"client\" class=\"org.acme.client.AcmeClient\"/>\n</beans>\n",
    ),
    ("django/__init__.py", ""),
    ("django/conf/__init__.py", ""),
    (
        "django/conf/global_settings.py",
        "DEFAULT_EXCEPTION_REPORTER = \"django.views.debug.ExceptionReporter\"\n",
    ),
    ("django/views/__init__.py", ""),
    (
        "django/views/debug.py",
        "class ExceptionReporter:\n    def get_traceback_text(self):\n        return \"\"\n",
    ),
    ("django/utils/__init__.py", ""),
    (
        "django/utils/module_loading.py",
        "def import_string(dotted_path):\n    return dotted_path\n",
    ),
    ("django/core/__init__.py", ""),
    ("django/core/handlers/__init__.py", ""),
    (
        "django/core/handlers/wsgi.py",
        "class WSGIHandler:\n    def __init__(self):\n        pass\n",
    ),
    ("django/core/servers/__init__.py", ""),
    (
        "django/core/servers/basehttp.py",
        "from django.utils.module_loading import import_string\n\n\ndef get_internal_wsgi_application():\n    return import_string(\"django.core.handlers.wsgi.WSGIHandler\")\n",
    ),
    ("django/core/management/__init__.py", ""),
    ("django/core/management/commands/__init__.py", ""),
    (
        "django/core/management/commands/runserver.py",
        "from django.core.servers.basehttp import get_internal_wsgi_application\n\n\nclass Command:\n    def get_handler(self):\n        return get_internal_wsgi_application()\n",
    ),
];

async fn connect() -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_with_toolsets_for_test(&Toolsets::all())
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &Client, tool: &str, args: Value) -> String {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
        .expect("no protocol error");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert_ne!(result.is_error, Some(true), "{tool}: {text}");
    text
}

/// The section that starts with `heading`, up to the next blank line.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("no {heading:?} section: {text}"));
    let rest = &text[start..];
    rest.split("\n\n").next().unwrap_or(rest)
}

#[tokio::test]
async fn tools_list_the_places_that_name_a_symbol() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    std::env::set_var("HOME", data.path());
    let projects = tempfile::tempdir().unwrap();
    let project = projects.path().join("names");
    for (path, text) in FILES {
        let path = project.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let project = project.canonicalize().unwrap();
    let repo = myceliums_core::analyzer::repo_id_from_path(&project);
    MyceliumsMcp::new()
        .analyze_for_test(AnalyzeParams {
            path: project.to_string_lossy().into_owned(),
            skip_embeddings: Some(true),
            ..Default::default()
        })
        .await
        .expect("analyze");
    let client = connect().await;

    // find_callers: no caller, but three places name the class.
    let callers = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "AcmeClient"}),
    )
    .await;
    assert!(callers.contains("0 callers of AcmeClient."), "{callers}");
    let auto = "src/main/java/org/acme/autoconfigure/AcmeAutoConfiguration.java";
    assert_eq!(
        section(&callers, "Referenced by name"),
        format!(
            "Referenced by name (3: strings and config files naming it):\n\
             {auto}\n  \
             :3 in AcmeAutoConfiguration: @ConditionalOnClass(name = \"org.acme.client.AcmeClient\")\n  \
             :6 in AcmeAutoConfiguration.client: return Class.forName(\"org.acme.client.AcmeClient\");\n\
             src/main/resources/beans.xml\n  \
             :2 <bean id=\"client\" class=\"org.acme.client.AcmeClient\"/>\n"
        )
    );
    // `detail: full` stays the JSON it was.
    let full = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "AcmeClient", "detail": "full"}),
    )
    .await;
    serde_json::from_str::<Value>(&full).expect("JSON");

    // impact_of_change: the auto-configuration is named by its imports file.
    let impact = call(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "symbol_name": "AcmeAutoConfiguration"}),
    )
    .await;
    assert!(
        section(&impact, "Referenced by name").contains(
            "src/main/resources/META-INF/spring/org.springframework.boot.autoconfigure.AutoConfiguration.imports\n  :1 org.acme.autoconfigure.AcmeAutoConfiguration\n"
        ),
        "{impact}"
    );
    let reporter = call(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "symbol_name": "ExceptionReporter"}),
    )
    .await;
    assert!(
        reporter.contains(
            "django/conf/global_settings.py\n  :1 DEFAULT_EXCEPTION_REPORTER = \"django.views.debug.ExceptionReporter\"\n"
        ),
        "{reporter}"
    );

    // find_callees: the function loads WSGIHandler by name.
    let callees = call(
        &client,
        "find_callees",
        json!({"repo_id": repo, "symbol_name": "get_internal_wsgi_application"}),
    )
    .await;
    assert!(
        section(&callees, "Names by qualified name").contains(
            "django/core/handlers/wsgi.py\n  WSGIHandler Class :1-3 (named at django/core/servers/basehttp.py:5)\n"
        ),
        "{callees}"
    );

    // trace_flow: runserver reaches WSGIHandler only by name. No CALLS
    // chain, but the answer names the step.
    let flow = call(
        &client,
        "trace_flow",
        json!({"repo_id": repo, "from_symbol": "Command.get_handler", "to_symbol": "WSGIHandler"}),
    )
    .await;
    assert!(
        section(&flow, "Names by qualified name").contains("WSGIHandler Class :1-3 (named at django/core/servers/basehttp.py:5 in get_internal_wsgi_application)"),
        "{flow}"
    );
    assert!(
        section(&flow, "Referenced by name").contains(
            "django/core/servers/basehttp.py\n  :5 in get_internal_wsgi_application: return import_string(\"django.core.handlers.wsgi.WSGIHandler\")\n"
        ),
        "{flow}"
    );

    // Answers about symbols nobody names by a string have no section.
    let plain = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "get_internal_wsgi_application"}),
    )
    .await;
    assert!(!plain.contains("Referenced by name"), "{plain}");
}
