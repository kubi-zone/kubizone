use std::time::Duration;

use futures::Future;
use k8s_openapi::{
    api::core::v1::Namespace,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
    serde::de::DeserializeOwned,
};
use kube::{
    api::{DeleteParams, ObjectMeta, PostParams},
    runtime::{
        conditions,
        wait::{await_condition, Condition},
    },
    Api, Client, CustomResourceExt, Resource, ResourceExt as _,
};
use kubizone::{record::RecordControllerContext, zone::ZoneControllerContext};
use kubizone_crds::v1alpha1::{DomainExt, Record, Zone};
use tracing::{debug, error, info};

pub struct Check<R> {
    pub name: String,
    pub func: Box<dyn Fn(&R) -> Result<(), String> + Send + Sync>,
}

impl<R> Check<R> {
    pub fn new(
        name: &str,
        func: impl Fn(&R) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Check {
            name: name.into(),
            func: Box::new(func),
        }
    }

    pub fn perform(&self, resource: &R) -> Result<(), String> {
        if let Err(err) = (self.func)(resource) {
            Err(format!("failed on: {}: {}", self.name, err))
        } else {
            Ok(())
        }
    }
}

#[allow(dead_code)]
pub fn has_serial() -> Check<Zone> {
    Check::new("has-serial", move |zone: &Zone| {
        if zone.status.iter().any(|status| status.serial.is_some()) {
            Ok(())
        } else {
            Err("not present".to_string())
        }
    })
}

#[allow(dead_code)]
pub fn has_entry(fqdn: &str) -> Check<Zone> {
    let fqdn = fqdn.to_string();

    Check::new("has-entry", move |zone: &Zone| {
        if zone.status.iter().any(|status| {
            !status.entries.is_empty()
                && status
                    .entries
                    .iter()
                    .any(|entry| !entry.type_.is_soa() && &entry.fqdn == fqdn.as_str())
        }) {
            Ok(())
        } else {
            Err("not present".to_string())
        }
    })
}

#[allow(dead_code)]
pub fn has_parent<R: DomainExt>(parent: &str) -> Check<R> {
    let parent = parent.to_string();
    Check::new("has-parent", move |resource: &R| {
        let Some(label) = resource.parent() else {
            return Err("no parent".to_string());
        };

        if label.as_label() != parent {
            return Err(format!(
                r#"wrong parent got "{label}", expected "{parent}""#,
                label = label.as_label(),
            ));
        }

        Ok(())
    })
}

#[allow(dead_code)]
pub fn not<R: 'static>(inner: Check<R>) -> Check<R> {
    Check {
        name: format!("not {}", inner.name),
        func: Box::new(move |resource: &R| match (inner.func)(resource) {
            Err(_) => Ok(()),
            Ok(()) => Err("was true".to_string()),
        }),
    }
}

#[allow(dead_code)]
pub async fn wait_for<R>(api: &Api<R>, name: &str, checks: &[Check<R>]) -> Result<R, ()>
where
    R: Resource + Clone + std::fmt::Debug + DeserializeOwned,
{
    'retry: for _ in 0..100 {
        let resource = api.get(name).await.unwrap();

        for check in checks.iter() {
            if let Err(err) = check.perform(&resource) {
                debug!("check {err}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue 'retry;
            }
        }

        info!(
            "ok {}: {}",
            name,
            checks
                .iter()
                .map(|check| check.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return Ok(resource);
    }

    error!(
        "timeout {}: {}",
        name,
        checks
            .iter()
            .map(|check| check.name.clone())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Err(())
}

/// Creates a namespace for the tests to run in, and starts the controllers.
pub async fn run<F: Future<Output = ()> + Send + 'static>(
    name: &str,
    func: impl Fn(Client, Namespace) -> F + Send + 'static,
) {
    tracing_subscriber::fmt::init();
    let client = Client::try_default().await.unwrap();

    let namespaces = Api::<Namespace>::all(client.clone());
    namespaces
        .delete(name, &DeleteParams::foreground())
        .await
        .ok();

    let namespace = namespaces
        .create(
            &PostParams::default(),
            &Namespace {
                metadata: ObjectMeta {
                    name: Some(name.to_string()),
                    ..Default::default()
                },
                spec: None,
                status: None,
            },
        )
        .await
        .unwrap();

    recreate_crds_destructively(client.clone()).await;

    let controller_client = client.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = kubizone::zone::controller(ZoneControllerContext { client: controller_client.clone(), requeue_time: Duration::from_secs(1) }) => (),
            _ = kubizone::record::controller(RecordControllerContext { client: controller_client.clone(), requeue_time: Duration::from_secs(1) }) => ()
        }
    });

    let namespace_clone = namespace.clone();
    let outcome = tokio::spawn(async move { func(client, namespace_clone).await }).await;
    namespaces
        .delete(&namespace.name_any(), &DeleteParams::foreground())
        .await
        .unwrap();

    outcome.unwrap();
}

async fn destroy_crd<C>(client: Client)
where
    C: Resource<DynamicType = ()> + CustomResourceExt,
{
    let api = Api::<CustomResourceDefinition>::all(client);

    let api_clone = api.clone();
    let watcher = tokio::spawn(async move {
        tokio::time::timeout(
            Duration::from_secs(30),
            await_condition(
                api_clone,
                C::crd_name(),
                conditions::is_crd_established().not(),
            ),
        )
        .await
        .unwrap()
        .unwrap()
    });

    api.delete(C::crd_name(), &DeleteParams::default())
        .await
        .ok();

    watcher.await.unwrap();
}

async fn create_crd<C>(client: Client)
where
    C: Resource<DynamicType = ()> + CustomResourceExt,
{
    let api = Api::<CustomResourceDefinition>::all(client);

    api.create(&PostParams::default(), &C::crd()).await.unwrap();

    tokio::time::timeout(
        Duration::from_secs(30),
        await_condition(api, C::crd_name(), conditions::is_crd_established()),
    )
    .await
    .unwrap()
    .unwrap();
}

async fn recreate_crds_destructively(client: Client) {
    destroy_crd::<Record>(client.clone()).await;
    destroy_crd::<Zone>(client.clone()).await;
    create_crd::<Zone>(client.clone()).await;
    create_crd::<Record>(client.clone()).await;
}
