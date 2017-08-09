/*! Asynchronous http client. */

use std::ops::Deref;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::Value;
use reqwest::unstable::async::{Client, RequestBuilder, Response, Body};
use futures::{Future, Stream, IntoFuture};
use tokio_core::reactor::Handle;

use super::req::HttpRequest;
use super::res::parsing::{Parse, IsOk};
use super::{Error, RequestParams, build_url, build_method};

/** Get a default `Client` and `RequestParams`. */
pub fn default(handle: &Handle) -> Result<(Client, RequestParams), Error> {
    Client::new(handle)
        .map(|cli| (cli, RequestParams::default()))
        .map_err(Into::into)
}

/** A type that can be converted into a request body. */
pub struct AsyncBody(Body);

impl AsyncBody {
    /** Convert the body into its inner value. */
    pub fn into_inner(self) -> Body {
        self.0
    }
}

impl Deref for AsyncBody {
    type Target = Body;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Body> for AsyncBody {
    fn from(body: Body) -> AsyncBody {
        AsyncBody(body)
    }
}

impl From<Vec<u8>> for AsyncBody {
    fn from(body: Vec<u8>) -> AsyncBody {
        AsyncBody(body.into())
    }
}

impl From<String> for AsyncBody {
    fn from(body: String) -> AsyncBody {
        AsyncBody(body.into())
    }
}

impl From<Value> for AsyncBody {
    fn from(body: Value) -> AsyncBody {
        AsyncBody(body.to_string().into())
    }
}

impl From<&'static [u8]> for AsyncBody {
    fn from(body: &'static [u8]) -> AsyncBody {
        AsyncBody(Bytes::from(body).into())
    }
}

impl From<&'static str> for AsyncBody {
    fn from(body: &'static str) -> AsyncBody {
        AsyncBody(Bytes::from(body).into())
    }
}

/** Represents a client that can send Elasticsearch requests asynchronously. */
pub trait AsyncElasticClient {
    /** 
    Send a request and get a response.
    
    # Examples
    
    Bring the `AsyncElasticClient` trait into scope and call `elastic_req` with any type that can be converted into a `req::HttpRequest`.
    This method returns a raw `reqwest::Response`.
    
    ```no_run
    # extern crate elastic_reqwest;
    # extern crate tokio_core;
    # use elastic_reqwest::req::SimpleSearchRequest;
    # fn main() {
    # let mut core = tokio_core::reactor::Core::new().unwrap();
    # let request = SimpleSearchRequest::for_index_ty("myindex", "mytype");
    use elastic_reqwest::AsyncElasticClient;
    
    let (client, params) = elastic_reqwest::async::default(&core.handle()).unwrap();
    
    let http_future = client.elastic_req(&params, request);

    core.run(http_future).unwrap();
    # }
    ```
    */
    fn elastic_req<I, B>(&self, params: &RequestParams, req: I) -> Box<Future<Item = Response, Error = Error>>
        where I: Into<HttpRequest<'static, B>>,
              B: Into<AsyncBody>;
}

/** Build an asynchronous `reqwest::RequestBuilder` from an Elasticsearch request. */
pub fn build_req<I, B>(client: &Client, params: &RequestParams, req: I) -> Result<RequestBuilder, Error>
    where I: Into<HttpRequest<'static, B>>,
          B: Into<AsyncBody>
{
    let req = req.into();

    let url = build_url(&req.url, &params);
    let method = build_method(req.method);
    let body = req.body;

    let mut req = client.request(method, &url)?;
    {
        req.headers(params.get_headers());

        if let Some(body) = body {
            req.body(body.into().into_inner());
        }
    }

    Ok(req)
}

impl AsyncElasticClient for Client {
    fn elastic_req<I, B>(&self, params: &RequestParams, req: I) -> Box<Future<Item = Response, Error = Error>>
        where I: Into<HttpRequest<'static, B>>,
              B: Into<AsyncBody>
    {
        let fut = build_req(&self, params, req)
            .into_future()
            .and_then(|mut req| req.send().map_err(Into::into));

        Box::new(fut)
    }
}

/** Represents a response that can be parsed into a concrete Elasticsearch response. */
pub trait AsyncFromResponse<TResponse> {
    /** Parse a response into a concrete response type. */
    fn from_response(self, response: Response) -> Box<Future<Item = TResponse, Error = Error>>;
}

impl<TResponse: IsOk + DeserializeOwned + 'static> AsyncFromResponse<TResponse> for Parse<TResponse> {
    fn from_response(self, mut response: Response) -> Box<Future<Item = TResponse, Error = Error>> {
        let status: u16 = response.status().into();
        let body_future = response.body().concat2()
            .map_err(Into::into);

        let de_future = body_future
            .and_then(move |body| {
                self.from_slice(status, body.as_ref()).map_err(Into::into)
            });

        Box::new(de_future)
    }
}

#[cfg(test)]
mod tests {
    use reqwest::Method;
    use reqwest::unstable::async::{Client, RequestBuilder};
    use reqwest::header::ContentType;
    use tokio_core::reactor::Core;

    use super::*;
    use req::*;

    fn params() -> RequestParams {
        RequestParams::new("eshost:9200/path")
            .url_param("pretty", true)
            .url_param("q", "*")
    }

    fn expected_req(cli: &Client, method: Method, url: &str, body: Option<Vec<u8>>) -> RequestBuilder {
        let mut req = cli.request(method, url).unwrap();
        {
            req.header(ContentType::json());

            if let Some(body) = body {
                req.body(body);
            }
        }

        req
    }

    fn assert_req(expected: RequestBuilder, actual: RequestBuilder) {
        assert_eq!(format!("{:?}", expected), format!("{:?}", actual));
    }

    fn core() -> Core {
        Core::new().unwrap()
    }

    #[test]
    fn head_req() {
        let cli = Client::new(&core().handle()).unwrap();
        let req = build_req(&cli, &params(), PingHeadRequest::new());

        let url = "eshost:9200/path/?pretty=true&q=*";

        let expected = expected_req(&cli, Method::Head, url, None);

        assert_req(expected, req.unwrap());
    }

    #[test]
    fn get_req() {
        let cli = Client::new(&core().handle()).unwrap();
        let req = build_req(&cli, &params(), SimpleSearchRequest::new());

        let url = "eshost:9200/path/_search?pretty=true&q=*";

        let expected = expected_req(&cli, Method::Get, url, None);

        assert_req(expected, req.unwrap());
    }

    #[test]
    fn post_req() {
        let cli = Client::new(&core().handle()).unwrap();
        let req = build_req(&cli,
                            &params(),
                            PercolateRequest::for_index_ty("idx", "ty", vec![]));

        let url = "eshost:9200/path/idx/ty/_percolate?pretty=true&q=*";

        let expected = expected_req(&cli, Method::Post, url, Some(vec![]));

        assert_req(expected, req.unwrap());
    }

    #[test]
    fn put_req() {
        let cli = Client::new(&core().handle()).unwrap();
        let req = build_req(&cli,
                            &params(),
                            IndicesCreateRequest::for_index("idx", vec![]));

        let url = "eshost:9200/path/idx?pretty=true&q=*";

        let expected = expected_req(&cli, Method::Put, url, Some(vec![]));

        assert_req(expected, req.unwrap());
    }

    #[test]
    fn delete_req() {
        let cli = Client::new(&core().handle()).unwrap();
        let req = build_req(&cli, &params(), IndicesDeleteRequest::for_index("idx"));

        let url = "eshost:9200/path/idx?pretty=true&q=*";

        let expected = expected_req(&cli, Method::Delete, url, None);

        assert_req(expected, req.unwrap());
    }

    #[test]
    fn owned_string_into_body() {
        AsyncBody::from(String::new());
    }

    #[test]
    fn borrowed_string_into_body() {
        AsyncBody::from("abc");
    }

    #[test]
    fn owned_vec_into_body() {
        AsyncBody::from(Vec::new());
    }

    #[test]
    fn borrowed_vec_into_body() {
        static BODY: &'static [u8] = &[0, 1, 2];

        AsyncBody::from(BODY);
    }

    #[test]
    fn empty_body_into_body() {
        AsyncBody::from(empty_body());
    }

    #[test]
    fn json_value_into_body() {
        AsyncBody::from(json!({}));
    }
}
