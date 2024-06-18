use std::{sync::Arc, time::Duration};

use futures::Future;
use k8s_openapi::{
    api::core::v1::Namespace,
    apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition,
    serde::de::DeserializeOwned, NamespaceResourceScope,
};
use kube::{
    api::{DeleteParams, ObjectMeta, PostParams},
    runtime::{
        conditions,
        wait::{await_condition, Condition},
    },
    Api, Client, CustomResourceExt, Resource, ResourceExt,
};
use kubizone::{record::RecordControllerContext, zone::ZoneControllerContext};
use kubizone_common::DomainName;
use kubizone_crds::v1alpha1::{Delegation, DomainExt, Record, RecordSpec, Zone, ZoneSpec};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

#[derive(Clone)]
pub struct ContextInner {
    namespaces: Vec<Namespace>,
    records: Vec<Record>,
    zones: Vec<Zone>,
    client: Client,
}

#[derive(Clone)]
pub struct Context {
    inner: Arc<RwLock<ContextInner>>,
}

impl Context {
    pub async fn namespace(&self, name: &str) -> Result<Namespace, kube::Error> {
        let api = Api::<Namespace>::all(self.inner.read().await.client.clone());

        // Try deleting it, in case it already exists.
        api.delete(name, &DeleteParams::foreground()).await.ok();

        let namespace = api
            .create(
                &PostParams::default(),
                &Namespace {
                    metadata: ObjectMeta {
                        name: Some(name.to_string()),
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .await?;

        self.inner.write().await.namespaces.push(namespace.clone());

        Ok(namespace)
    }

    pub async fn record(
        &self,
        namespace: &str,
        name: &str,
        fqdn: &str,
    ) -> Result<Record, kube::Error> {
        let api = Api::<Record>::namespaced(self.inner.read().await.client.clone(), namespace);

        let record = api
            .create(
                &PostParams::default(),
                &Record {
                    metadata: ObjectMeta {
                        name: Some(name.to_string()),
                        ..Default::default()
                    },
                    spec: RecordSpec {
                        domain_name: DomainName::try_from(fqdn).unwrap(),
                        zone_ref: None,
                        type_: kubizone_common::Type::A,
                        class: kubizone_common::Class::IN,
                        ttl: None,
                        rdata: "127.0.0.1".to_string(),
                    },
                    status: None,
                },
            )
            .await?;

        self.inner.write().await.records.push(record.clone());
        Ok(record)
    }

    pub async fn zone(
        &self,
        namespace: &str,
        name: &str,
        fqdn: &str,
        delegations: &[Delegation],
    ) -> Result<Zone, kube::Error> {
        let api = Api::<Zone>::namespaced(self.inner.read().await.client.clone(), namespace);

        let zone = api
            .create(
                &PostParams::default(),
                &Zone {
                    metadata: ObjectMeta {
                        name: Some(name.to_string()),
                        ..Default::default()
                    },
                    spec: ZoneSpec {
                        domain_name: DomainName::try_from(fqdn.to_string()).unwrap(),
                        delegations: delegations.to_vec(),
                        ..Default::default()
                    },
                    status: None,
                },
            )
            .await?;

        self.inner.write().await.zones.push(zone.clone());
        Ok(zone)
    }

    pub async fn wait_for<R>(&self, resource: &R, checks: &[Check<R>]) -> Result<R, ()>
    where
        R: Resource<Scope = NamespaceResourceScope> + Clone + std::fmt::Debug + DeserializeOwned,
        <R as Resource>::DynamicType: Default,
    {
        let client = self.inner.read().await.client.clone();

        let api = Api::<R>::namespaced(client, resource.meta().namespace.as_ref().unwrap());
        let name = resource.name_any();

        'retry: for _ in 0..100 {
            let resource = api.get(&name).await.unwrap();

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

    async fn clean(self) {
        let mut inner = self.inner.write().await;
        let client = inner.client.clone();

        for record in inner.records.drain(..) {
            let api = Api::<Record>::namespaced(
                client.clone(),
                record.meta().namespace.as_ref().unwrap(),
            );
            api.delete(&record.name_any(), &DeleteParams::foreground())
                .await
                .ok();
        }

        for zone in inner.zones.drain(..) {
            let api =
                Api::<Zone>::namespaced(client.clone(), zone.meta().namespace.as_ref().unwrap());
            api.delete(&zone.name_any(), &DeleteParams::foreground())
                .await
                .ok();
        }

        for namespace in inner.namespaces.drain(..) {
            let api = Api::<Namespace>::all(client.clone());
            api.delete(&namespace.name_any(), &DeleteParams::foreground())
                .await
                .ok();
        }
    }
}

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
pub fn has_parent<R: DomainExt>(parent: &Zone) -> Check<R> {
    let parent = parent.zone_ref();
    Check::new("has-parent", move |resource: &R| {
        let Some(label) = resource.parent() else {
            return Err("no parent".to_string());
        };

        if label != parent {
            return Err(format!(
                r#"wrong parent got "{label}", expected "{parent}""#,
            ));
        }

        Ok(())
    })
}

#[allow(dead_code)]
pub fn has_fqdn<R: DomainExt>() -> Check<R> {
    Check::new("has-fqdn", move |resource: &R| match resource.fqdn() {
        Some(_) => Ok(()),
        None => Err("no parent".to_string()),
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

/// Creates a namespace for the tests to run in, and starts the controllers.
pub async fn run<F: Future<Output = ()> + Send + 'static>(
    func: impl Fn(Context) -> F + Send + 'static,
) {
    tracing_subscriber::fmt::init();
    let client = Client::try_default().await.unwrap();

    recreate_crds_destructively(client.clone()).await;

    let context = Context {
        inner: Arc::new(RwLock::new(ContextInner {
            namespaces: vec![],
            records: vec![],
            zones: vec![],
            client: client.clone(),
        })),
    };

    let controller_client = client.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = kubizone::zone::controller(ZoneControllerContext { client: controller_client.clone(), requeue_time: Duration::from_secs(1) }) => (),
            _ = kubizone::record::controller(RecordControllerContext { client: controller_client.clone(), requeue_time: Duration::from_secs(1) }) => ()
        }
    });

    let context_clone = context.clone();
    let outcome = tokio::spawn(async move { func(context_clone).await }).await;

    context.clean().await;

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
