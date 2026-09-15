use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query};
use axum::http::{Method, Request, Response, StatusCode, header};
use axum::{Json, Router};
use incan_web_macros::route;
use serde::{Deserialize, Serialize};
use tower::ServiceExt;

#[derive(Debug, Deserialize)]
struct Search {
    q: String,
}

#[derive(Debug, Deserialize)]
struct Update {
    name: String,
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
struct Reply {
    value: String,
}

#[route("/__incan_route_test/search", methods = ["GET"])]
async fn query_handler(query: Query<Search>) -> Json<Reply> {
    Json(Reply { value: query.q.clone() })
}

#[route("/__incan_route_test/json", methods = ["POST"])]
async fn json_handler(body: Json<Update>) -> Json<Reply> {
    Json(Reply {
        value: body.name.clone(),
    })
}

#[route("/__incan_route_test/typed/{id}", methods = ["GET"])]
async fn typed_path_handler(id: Path<i64>) -> Json<Reply> {
    Json(Reply {
        value: id.0.to_string(),
    })
}

#[route("/__incan_route_test/unused/{id}", methods = ["GET"])]
async fn unused_typed_path_handler(_: Path<i64>) -> Json<Reply> {
    Json(Reply {
        value: "unused".to_string(),
    })
}

#[route("/__incan_route_test/scalar/{id}", methods = ["GET"])]
async fn scalar_path_handler(id: i64) -> Json<Reply> {
    Json(Reply { value: id.to_string() })
}

#[route("/__incan_route_test/multi/{year}/{month}", methods = ["GET"])]
async fn multiple_path_handler(year: i64, month: i64) -> Json<Reply> {
    Json(Reply {
        value: format!("{year}-{month}"),
    })
}

#[route("/__incan_route_test/mixed/{id}", methods = ["POST"])]
async fn mixed_handler(id: i64, query: Query<Search>, body: Json<Update>) -> Json<Reply> {
    Json(Reply {
        value: format!("{id}:{}:{}", query.q, body.name),
    })
}

#[route("/__incan_route_test/methods", methods = ["GET", "POST"])]
async fn multiple_methods_handler() -> Json<Reply> {
    Json(Reply {
        value: "methods".to_string(),
    })
}

/// Build the same inventory-backed Axum router that `std.web.App.run()` serves.
fn test_router() -> Router {
    inventory::iter::<incan_stdlib::web::RouteEntry>
        .into_iter()
        .fold(Router::new(), |router, entry| (entry.register)(router))
}

/// Dispatch one request through the inventory-backed router.
async fn dispatch(method: Method, uri: &str, body: Option<&str>) -> Result<Response<Body>, Box<dyn std::error::Error>> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let request = builder.body(Body::from(body.unwrap_or_default().to_string()))?;
    Ok(test_router().oneshot(request).await?)
}

/// Decode one JSON route response through the public HTTP body.
async fn decode_reply(response: Response<Body>) -> Result<Reply, Box<dyn std::error::Error>> {
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[tokio::test]
async fn typed_and_scalar_extractors_execute_through_axum() -> Result<(), Box<dyn std::error::Error>> {
    let query = dispatch(Method::GET, "/__incan_route_test/search?q=typed", None).await?;
    assert_eq!(query.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(query).await?,
        Reply {
            value: "typed".to_string()
        }
    );

    let json = dispatch(Method::POST, "/__incan_route_test/json", Some(r#"{"name":"body"}"#)).await?;
    assert_eq!(json.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(json).await?,
        Reply {
            value: "body".to_string()
        }
    );

    let typed_path = dispatch(Method::GET, "/__incan_route_test/typed/41", None).await?;
    assert_eq!(typed_path.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(typed_path).await?,
        Reply {
            value: "41".to_string()
        }
    );

    let unused_path = dispatch(Method::GET, "/__incan_route_test/unused/42", None).await?;
    assert_eq!(unused_path.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(unused_path).await?,
        Reply {
            value: "unused".to_string()
        }
    );

    let scalar_path = dispatch(Method::GET, "/__incan_route_test/scalar/43", None).await?;
    assert_eq!(scalar_path.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(scalar_path).await?,
        Reply {
            value: "43".to_string()
        }
    );

    let multiple_path = dispatch(Method::GET, "/__incan_route_test/multi/2026/7", None).await?;
    assert_eq!(multiple_path.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(multiple_path).await?,
        Reply {
            value: "2026-7".to_string()
        }
    );

    let mixed = dispatch(
        Method::POST,
        "/__incan_route_test/mixed/44?q=query",
        Some(r#"{"name":"body"}"#),
    )
    .await?;
    assert_eq!(mixed.status(), StatusCode::OK);
    assert_eq!(
        decode_reply(mixed).await?,
        Reply {
            value: "44:query:body".to_string()
        }
    );

    for method in [Method::GET, Method::POST] {
        let response = dispatch(method, "/__incan_route_test/methods", None).await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            decode_reply(response).await?,
            Reply {
                value: "methods".to_string()
            }
        );
    }
    Ok(())
}

#[tokio::test]
async fn malformed_typed_extractors_return_client_errors() -> Result<(), Box<dyn std::error::Error>> {
    let query = dispatch(Method::GET, "/__incan_route_test/search", None).await?;
    assert_eq!(query.status(), StatusCode::BAD_REQUEST);

    let json = dispatch(Method::POST, "/__incan_route_test/json", Some("not-json")).await?;
    assert_eq!(json.status(), StatusCode::BAD_REQUEST);

    let typed_path = dispatch(Method::GET, "/__incan_route_test/typed/not-an-integer", None).await?;
    assert_eq!(typed_path.status(), StatusCode::BAD_REQUEST);

    let scalar_path = dispatch(Method::GET, "/__incan_route_test/scalar/not-an-integer", None).await?;
    assert_eq!(scalar_path.status(), StatusCode::BAD_REQUEST);
    Ok(())
}
