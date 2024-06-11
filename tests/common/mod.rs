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
use kubizone_crds::v1alpha1::{Record, Zone};

#[allow(dead_code)]
pub fn has_serial() -> Box<dyn Fn(&Zone) -> bool + Send + Sync> {
    Box::new(move |zone: &Zone| zone.status.iter().any(|status| status.serial.is_some()))
}

#[allow(dead_code)]
pub fn has_entry(fqdn: &str) -> Box<dyn Fn(&Zone) -> bool + Send + Sync> {
    let fqdn = fqdn.to_string();
    Box::new(move |zone: &Zone| {
        zone.status.iter().any(|status| {
            status
                .entries
                .iter()
                .any(|entry| &entry.fqdn == fqdn.as_str())
        })
    })
}

pub async fn wait_for<R>(
    api: Api<R>,
    name: &str,
    checks: &[Box<dyn Fn(&R) -> bool + Send + Sync>],
) -> bool
where
    R: Resource + Clone + std::fmt::Debug + DeserializeOwned,
{
    for _ in 0..10 {
        let resource = api.get(name).await.unwrap();

        for check in checks.iter() {
            if !check(&resource) {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }
        }

        return true;
    }

    false
}

/// Creates a namespace for the tests to run in, and starts the controllers.
pub async fn run<F: Future<Output = ()> + Send + 'static>(
    name: &str,
    func: impl Fn(Client, Namespace) -> F + Send + 'static,
) {
    let client = Client::try_default().await.unwrap();

    let namespaces = Api::<Namespace>::all(client.clone());
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
            _ = kubizone::zone::controller(controller_client.clone()) => (),
            _ = kubizone::record::controller(controller_client) => ()
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

    api.delete(C::crd_name(), &DeleteParams::default())
        .await
        .ok();

    tokio::time::timeout(
        Duration::from_secs(30),
        await_condition(api, C::crd_name(), conditions::is_crd_established().not()),
    )
    .await
    .unwrap()
    .unwrap();
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
