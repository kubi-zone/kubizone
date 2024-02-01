use futures::StreamExt;
use k8s_openapi::serde_json::json;
use std::{sync::Arc, time::Duration};

use kube::{
    api::{ListParams, Patch, PatchParams},
    runtime::{controller::Action, watcher, Controller},
    Api, Client, ResourceExt,
};
use kubizone_crds::{
    kubizone_common::{DomainName, FullyQualifiedDomainName},
    v1alpha1::{Record, Zone, ZoneRef},
    PARENT_ZONE_LABEL,
};
use tracing::*;

const CONTROLLER_NAME: &str = "kubi.zone/record-resolver";

pub async fn controller(client: Client) {
    let records = Api::<Record>::all(client.clone());

    let record_controller = Controller::new(records, watcher::Config::default())
        .watches(
            Api::<Zone>::all(client.clone()),
            watcher::Config::default(),
            kubizone_crds::watch_reference(PARENT_ZONE_LABEL),
        )
        .shutdown_on_signal()
        .run(
            reconcile_records,
            record_error_policy,
            Arc::new(Data {
                client: client.clone(),
            }),
        )
        .for_each(|res| async move {
            match res {
                Ok(o) => info!("reconciled {:?}", o),
                Err(e) => warn!("reconcile failed: {}", e),
            }
        });

    record_controller.await;
}

struct Data {
    client: Client,
}

async fn set_record_fqdn(
    client: Client,
    record: &Record,
    fqdn: &FullyQualifiedDomainName,
) -> Result<(), kube::Error> {
    if record
        .status
        .as_ref()
        .and_then(|status| status.fqdn.as_ref())
        != Some(fqdn)
    {
        info!("updating fqdn for record {} to {}", record.name_any(), fqdn);
        Api::<Record>::namespaced(client, record.namespace().as_ref().unwrap())
            .patch_status(
                &record.name_any(),
                &PatchParams::apply(CONTROLLER_NAME),
                &Patch::Merge(json!({
                    "status": {
                        "fqdn": fqdn,
                    }
                })),
            )
            .await?;
    } else {
        debug!("not updating fqdn for record {} {fqdn}", record.name_any())
    }
    Ok(())
}

async fn set_record_parent_ref(
    client: Client,
    record: &Arc<Record>,
    parent_ref: &ZoneRef,
) -> Result<(), kube::Error> {
    if record.labels().get(PARENT_ZONE_LABEL) != Some(&parent_ref.as_label()) {
        info!(
            "updating record {}'s {PARENT_ZONE_LABEL} to {parent_ref}",
            record.name_any()
        );
        Api::<Record>::namespaced(client, record.namespace().as_ref().unwrap())
            .patch_metadata(
                &record.name_any(),
                &PatchParams::apply(CONTROLLER_NAME),
                &Patch::Merge(json!({
                    "metadata": {
                        "labels": {
                            PARENT_ZONE_LABEL: parent_ref.as_label()
                        },
                    }
                })),
            )
            .await?;
    } else {
        debug!(
            "not updating record {record}'s {PARENT_ZONE_LABEL} since it is already {parent_ref}"
        )
    }
    Ok(())
}

async fn reconcile_records(record: Arc<Record>, ctx: Arc<Data>) -> Result<Action, kube::Error> {
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
                return Ok(Action::requeue(Duration::from_secs(30)));
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
                set_record_fqdn(ctx.client.clone(), &record, &alleged_fqdn).await?;
                set_record_parent_ref(ctx.client.clone(), &record, &parent_zone.zone_ref()).await?;
            } else {
                warn!("parent zone {parent_zone} was found, but its delegations does not allow adoption of {record} with {alleged_fqdn} and type {}", record.spec.type_);
                return Ok(Action::requeue(Duration::from_secs(300)));
            }
        }
        (None, DomainName::Full(record_fqdn)) => {
            set_record_fqdn(ctx.client.clone(), &record, record_fqdn).await?;

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
                    set_record_parent_ref(
                        ctx.client.clone(),
                        &record,
                        &longest_parent_zone.zone_ref(),
                    )
                    .await?;
                } else {
                    warn!("{longest_parent_zone} is the most immediate parent zone of {record}, but the zone's delegation rules do not allow the adoption of it.");
                }
            } else {
                warn!(
                    "record {record} ({}) does not fit into any found parent Zone",
                    &record.spec.domain_name
                );
            };
        }
        (Some(zone_ref), DomainName::Full(record_fqdn)) => {
            warn!("record {record} has both a fully qualified domain_name ({record_fqdn}) and a zoneRef({zone_ref}). It cannot have both.");
            return Ok(Action::requeue(Duration::from_secs(300)));
        }
        (None, DomainName::Partial(_)) => {
            warn!("{record} has neither zoneRef nor a fully qualified domainName, making it impossible to deduce its parent zone.");
            return Ok(Action::requeue(Duration::from_secs(300)));
        }
    }

    Ok(Action::requeue(Duration::from_secs(30)))
}

fn record_error_policy(record: Arc<Record>, error: &kube::Error, _ctx: Arc<Data>) -> Action {
    error!(
        "record {} reconciliation encountered error: {error}",
        record.name_any()
    );
    Action::requeue(Duration::from_secs(60))
}
