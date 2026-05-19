//! Integration test: verify the docs server serves the OpenAPI JSON spec.

use proxy::frameworks::openapi::build_docs_app;

#[tokio::test]
async fn docs_server_serves_openapi_spec() {
    // Start the docs server on an ephemeral port.
    let docs_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let docs_addr = docs_listener.local_addr().unwrap();

    let docs_app = build_docs_app();
    tokio::spawn(async move {
        axum::serve(docs_listener, docs_app).await.unwrap();
    });

    // Give the server a moment to start.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Fetch the OpenAPI JSON.
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{docs_addr}/api-docs/openapi.json"))
        .send()
        .await
        .expect("docs server request failed");
    assert!(
        resp.status().is_success(),
        "expected 200, got {}",
        resp.status()
    );

    let body: serde_json::Value = resp.json().await.expect("invalid JSON");
    assert_eq!(body["info"]["title"], "CLI Router Proxy API");
    assert!(body["paths"].is_object(), "paths should be an object");

    // Verify key endpoints exist in the spec.
    let paths = body["paths"].as_object().unwrap();
    assert!(paths.contains_key("/v1/messages"), "missing /v1/messages");
    assert!(
        paths.contains_key("/v1/chat/completions"),
        "missing /v1/chat/completions"
    );
    assert!(
        paths.contains_key("/admin/status"),
        "missing /admin/status"
    );
    assert!(paths.contains_key("/admin/config"), "missing /admin/config");

    // Verify Swagger UI HTML is served.
    let swagger_resp = client
        .get(format!("http://{docs_addr}/swagger-ui/"))
        .send()
        .await
        .expect("swagger-ui request failed");
    assert!(swagger_resp.status().is_success());
}

#[tokio::test]
async fn docs_server_serves_redoc() {
    let docs_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let docs_addr = docs_listener.local_addr().unwrap();

    let docs_app = build_docs_app();
    tokio::spawn(async move {
        axum::serve(docs_listener, docs_app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{docs_addr}/redoc"))
        .send()
        .await
        .expect("redoc request failed");
    assert!(
        resp.status().is_success(),
        "expected 200, got {}",
        resp.status()
    );
}
