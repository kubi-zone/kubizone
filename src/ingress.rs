use futures::StreamExt;
use k8s_openapi::api::networking::v1::Ingress;
use kubizone_common::{Class, DomainName, Type};
use std::{net::IpAddr, str::FromStr, sync::Arc, time::Duration};

use kube::{
    api::{ObjectMeta, PostParams},
    runtime::{controller::Action, watcher, Controller},
    Api, Client, Resource, ResourceExt,
};
use kubizone_crds::v1alpha1::{Record, RecordSpec};
use tracing::*;

pub async fn controller(context: IngressControllerContext) {
    let ingresses = Api::<Ingress>::all(context.client.clone());
    let records = Api::<Record>::all(context.client.clone());

    let ingress_controller = Controller::new(ingresses, watcher::Config::default())
        .owns(records, watcher::Config::default())
        .shutdown_on_signal()
        .run(reconcile_ingresses, ingress_error_policy, Arc::new(context))
        .for_each(|res| async move {
            match res {
                Ok((o, _)) => info!("reconciled {}.{}", o.name, o.namespace.unwrap_or_default()),
                Err(e) => warn!("reconcile failed: {}", e),
            }
        });

    ingress_controller.await;
    warn!("ingress controller exited");
}

pub struct IngressControllerContext {
    pub client: Client,
    pub requeue_time: Duration,
}

#[tracing::instrument(name = "ingress", skip_all)]
async fn reconcile_ingresses(
    ingress: Arc<Ingress>,
    ctx: Arc<IngressControllerContext>,
) -> Result<Action, kube::Error> {
    let Some(spec) = ingress.spec.as_ref() else {
        debug!("ingress has no spec (???), requeueing.");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let Some(rules) = spec.rules.as_ref() else {
        debug!("ingress contains no rules, requeueing.");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let Some(status) = ingress.status.as_ref() else {
        debug!("ingress contains no status, requeueing.");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let Some(lb) = status.load_balancer.as_ref() else {
        debug!("ingress status contains no loadBalancer segment, requeueing.");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let Some(ingresses) = lb.ingress.as_ref() else {
        debug!("ingress status load balancer contains no ingresses, requeueing.");
        return Ok(Action::requeue(ctx.requeue_time));
    };

    let (ipv4_addresses, ipv6_addresses): (Vec<IpAddr>, Vec<IpAddr>) = ingresses
        .iter()
        .filter_map(|ingress| ingress.ip.as_ref())
        .filter_map(|address| IpAddr::from_str(&address).ok())
        .partition(IpAddr::is_ipv4);

    let hostnames: Vec<_> = rules
        .iter()
        .filter_map(|rule| rule.host.as_deref())
        .map(DomainName::try_from)
        .filter_map(Result::ok)
        .collect();

    let records =
        Api::<Record>::namespaced(ctx.client.clone(), ingress.namespace().as_ref().unwrap());
    for hostname in hostnames.iter() {
        let metadata = |addr: &IpAddr| ObjectMeta {
            name: Some(format!(
                "{}-{}-{}",
                ingress.name_any(),
                hostname.to_string().replace(".", "-"),
                addr.to_canonical()
                    .to_string()
                    .replace(".", "-")
                    .replace(":", "-")
            )),
            owner_references: Some(vec![ingress.owner_ref(&()).unwrap()]),
            ..Default::default()
        };

        for ipv4 in ipv4_addresses.iter() {
            let metadata = metadata(ipv4);
            info!("creating record {:?}: {hostname} -> {ipv4}", metadata.name);
            records
                .create(
                    &PostParams::default(),
                    &Record {
                        metadata: metadata,
                        spec: RecordSpec {
                            domain_name: hostname.to_fully_qualified().into(),
                            zone_ref: None,
                            type_: Type::A,
                            class: Class::IN,
                            ttl: None,
                            rdata: ipv4.to_string(),
                        },
                        status: None,
                    },
                )
                .await?;
        }

        for ipv6 in ipv6_addresses.iter() {
            let metadata = metadata(ipv6);
            info!("creating record {:?}: {hostname} -> {ipv6}", metadata.name);
            records
                .create(
                    &PostParams::default(),
                    &Record {
                        metadata,
                        spec: RecordSpec {
                            domain_name: hostname.to_fully_qualified().into(),
                            zone_ref: None,
                            type_: Type::AAAA,
                            class: Class::IN,
                            ttl: None,
                            rdata: ipv6.to_string(),
                        },
                        status: None,
                    },
                )
                .await?;
        }
    }

    Ok(Action::requeue(ctx.requeue_time))
}

fn ingress_error_policy(
    ingress: Arc<Ingress>,
    error: &kube::Error,
    _ctx: Arc<IngressControllerContext>,
) -> Action {
    error!(
        "ingress {} reconciliation encountered error: {error}",
        ingress.name_any()
    );
    Action::requeue(Duration::from_secs(60))
}
