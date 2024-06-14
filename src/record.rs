use futures::StreamExt;
use std::{sync::Arc, time::Duration};

use kube::{
    api::ListParams,
    runtime::{controller::Action, watcher, Controller},
    Api, Client, ResourceExt,
};
use kubizone_crds::{
    kubizone_common::DomainName,
    v1alpha1::{DomainExt as _, Record, Zone},
    PARENT_ZONE_LABEL,
};
use tracing::*;

use crate::{set_fqdn, set_parent};

#[cfg(feature = "dev")]
const CONTROLLER_NAME: &str = "dev.kubi.zone/record-resolver";
#[cfg(not(feature = "dev"))]
const CONTROLLER_NAME: &str = "kubi.zone/record-resolver";

pub async fn controller(context: RecordControllerContext) {
    let records = Api::<Record>::all(context.client.clone());

    let record_controller = Controller::new(records, watcher::Config::default())
        .watches(
            Api::<Zone>::all(context.client.clone()),
            watcher::Config::default(),
            kubizone_crds::watch_reference(PARENT_ZONE_LABEL),
        )
        .shutdown_on_signal()
        .run(reconcile_records, record_error_policy, Arc::new(context))
        .for_each(|res| async move {
            match res {
                Ok(o) => info!("reconciled {:?}", o),
                Err(e) => warn!("reconcile failed: {}", e),
            }
        });

    record_controller.await;
}

pub struct RecordControllerContext {
    pub client: Client,
    pub requeue_time: Duration,
}

#[tracing::instrument(name = "record", skip_all)]
async fn reconcile_records(
    record: Arc<Record>,
    ctx: Arc<RecordControllerContext>,
) -> Result<Action, kube::Error> {
    match (record.spec.zone_ref.as_ref(), &record.spec.domain_name) {
        (Some(zone_ref), DomainName::Partial(partial_domain)) => {
            // Follow the zoneRef to the supposed parent zone, if it exists
            // or requeue later if it does not.
            let Some(parent_zone) = Api::<Zone>::namespaced(
                ctx.client.clone(),
                &zone_ref
                    .namespace
                    .as_ref()
                    .or(record.namespace().as_ref())
                    .cloned()
                    .unwrap(),
            )
            .get_opt(&zone_ref.name)
            .await?
            else {
                warn!("record {record} references unknown zone {zone_ref}");
                return Ok(Action::requeue(ctx.requeue_time));
            };

            // If the parent does not have a fully qualified domain name defined
            // yet, we can't check if the delegations provided by it are valid.
            // Postpone the reconcilliation until a later time, when the fqdn
            // has (hopefully) been determined.
            let Some(parent_fqdn) = parent_zone.fqdn() else {
                info!("parent zone {parent_zone} missing fqdn, requeuing.",);
                return Ok(Action::requeue(Duration::from_secs(5)));
            };

            // This is only "alleged", since we don't know yet if the referenced
            // zone's delegations allow the adoption.
            let alleged_fqdn = partial_domain.with_origin(parent_fqdn);

            trace!("record alleged fqdn: {partial_domain} + {parent_fqdn} = {alleged_fqdn}");

            if parent_zone.spec.delegations.iter().any(|delegation| {
                delegation.covers_namespace(record.namespace().as_deref().unwrap())
                    && delegation.validate_record(parent_fqdn, record.spec.type_, &alleged_fqdn)
            }) {
                set_fqdn(CONTROLLER_NAME, ctx.client.clone(), &record, &alleged_fqdn).await?;
                set_parent(
                    &CONTROLLER_NAME,
                    ctx.client.clone(),
                    &record,
                    Some(parent_zone.zone_ref()),
                )
                .await?;
            } else {
                warn!("parent zone {parent_zone} was found, but its delegations does not allow adoption of {record} with {alleged_fqdn} and type {}", record.spec.type_);
                return Ok(Action::requeue(ctx.requeue_time));
            }
        }
        (None, DomainName::Full(record_fqdn)) => {
            if set_fqdn(&CONTROLLER_NAME, ctx.client.clone(), &record, record_fqdn)
                .await?
                .changed()
            {
                info!("record {record} fqdn changed to {record_fqdn}, requeuing.");
                return Ok(Action::requeue(Duration::from_secs(1)));
            }

            // Fetch all zones from across the cluster and then filter down results to only parent
            // zones which are valid parent zones for this one.
            //
            // This means filtering out parent zones without fqdns, as well as ones which do not
            // have appropriate delegations for our `zone`'s namespace and suffix.
            if let Some(longest_parent_zone) = Api::<Zone>::all(ctx.client.clone())
                .list(&ListParams::default())
                .await?
                .into_iter()
                .filter(|parent| {
                    parent
                        .fqdn()
                        .is_some_and(|parent_fqdn| record_fqdn.is_subdomain_of(parent_fqdn))
                })
                .max_by_key(|parent| parent.fqdn().unwrap().as_ref().len())
            {
                if longest_parent_zone.validate_record(&record) {
                    set_parent(
                        &CONTROLLER_NAME,
                        ctx.client.clone(),
                        &record,
                        Some(longest_parent_zone.zone_ref()),
                    )
                    .await?;
                } else {
                    warn!("{longest_parent_zone} is the most immediate parent zone of {record}, but the zone's delegation rules do not allow the adoption of it.");
                    set_parent(&CONTROLLER_NAME, ctx.client.clone(), &record, None).await?;
                }
            } else {
                warn!(
                    "record {record} ({}) does not fit into any found parent Zone",
                    &record.spec.domain_name
                );
                set_parent(&CONTROLLER_NAME, ctx.client.clone(), &record, None).await?;
            };
        }
        (Some(zone_ref), DomainName::Full(record_fqdn)) => {
            warn!("record {record} has both a fully qualified domain_name ({record_fqdn}) and a zoneRef({zone_ref}). It cannot have both.");
            return Ok(Action::requeue(ctx.requeue_time));
        }
        (None, DomainName::Partial(_)) => {
            warn!("{record} has neither zoneRef nor a fully qualified domainName, making it impossible to deduce its parent zone.");
            return Ok(Action::requeue(ctx.requeue_time));
        }
    }

    Ok(Action::requeue(ctx.requeue_time))
}

fn record_error_policy(
    record: Arc<Record>,
    error: &kube::Error,
    _ctx: Arc<RecordControllerContext>,
) -> Action {
    error!(
        "record {} reconciliation encountered error: {error}",
        record.name_any()
    );
    Action::requeue(Duration::from_secs(60))
}
