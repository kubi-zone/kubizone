use std::{
    collections::{hash_map::DefaultHasher, VecDeque},
    hash::{Hash, Hasher},
    sync::Arc,
    time::Duration,
};

use futures::StreamExt;
use k8s_openapi::serde_json::json;
use kube::{
    api::{ListParams, Patch, PatchParams},
    runtime::{controller::Action, watcher, Controller},
    Api, Client, ResourceExt,
};
use kubizone_crds::{
    v1alpha1::{Record, Zone, ZoneEntry, ZoneRef, ZoneSpec},
    PARENT_ZONE_LABEL,
};

use tracing::log::*;

struct Data {
    client: Client,
}

const CONTROLLER_NAME: &str = "kubi.zone/zone-resolver";

pub async fn controller(client: Client) {
    let zones = Api::<Zone>::all(client.clone());

    let zone_controller = Controller::new(zones.clone(), watcher::Config::default())
        .watches(
            Api::<Zone>::all(client.clone()),
            watcher::Config::default(),
            kubizone_crds::watch_reference(PARENT_ZONE_LABEL),
        )
        .watches(
            Api::<Record>::all(client.clone()),
            watcher::Config::default(),
            kubizone_crds::watch_reference(PARENT_ZONE_LABEL),
        )
        .shutdown_on_signal()
        .run(
            reconcile_zones,
            zone_error_policy,
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

    zone_controller.await;
}

async fn reconcile_zones(zone: Arc<Zone>, ctx: Arc<Data>) -> Result<Action, kube::Error> {
    match (
        zone.spec.zone_ref.as_ref(),
        zone.spec.domain_name.ends_with('.'),
    ) {
        (Some(zone_ref), false) => {
            // Follow the zoneRef to the supposed parent zone, if it exists
            // or requeue later if it does not.
            let Some(parent_zone) = Api::<Zone>::namespaced(
                ctx.client.clone(),
                &zone_ref
                    .namespace
                    .as_ref()
                    .or(zone.namespace().as_ref())
                    .cloned()
                    .unwrap(),
            )
            .get_opt(&zone_ref.name)
            .await?
            else {
                warn!("zone {zone} references unknown zone {zone_ref}");
                return Ok(Action::requeue(Duration::from_secs(30)));
            };

            // If the parent does not have a fully qualified domain name defined
            // yet, we can't check if the delegations provided by it are valid.
            // Postpone the reconcilliation until a later time, when the fqdn
            // has (hopefully) been determined.
            let Some(parent_fqdn) = parent_zone.fqdn() else {
                info!(
                    "parent zone {} missing fqdn, requeuing.",
                    parent_zone.name_any()
                );
                return Ok(Action::requeue(Duration::from_secs(5)));
            };

            // This is only "alleged", since we don't know yet if the referenced
            // zone's delegations allow the adoption.
            let alleged_fqdn = format!("{}.{}", zone.spec.domain_name, parent_fqdn);

            if parent_zone.spec.delegations.iter().any(|delegation| {
                delegation.covers_namespace(zone.namespace().as_deref().unwrap())
                    && delegation.validate_zone(parent_fqdn, &alleged_fqdn)
            }) {
                set_zone_fqdn(ctx.client.clone(), &zone, &alleged_fqdn).await?;
                set_zone_parent_ref(ctx.client.clone(), &zone, parent_zone.zone_ref()).await?;
            } else {
                warn!("parent zone {parent_zone} was found, but its delegations does not allow adoption of {zone} with {alleged_fqdn}");
                return Ok(Action::requeue(Duration::from_secs(300)));
            }
        }
        (None, true) => {
            set_zone_fqdn(ctx.client.clone(), &zone, &zone.spec.domain_name).await?;

            // Fetch all zones from across the cluster and then filter down results to only parent
            // zones which are valid parent zones for this one.
            //
            // This means filtering out parent zones without fqdns, as well as ones which do not
            // have appropriate delegations for our `zone`'s namespace and suffix.
            if let Some(longest_parent_zone) = Api::<Zone>::all(ctx.client.clone())
                .list(&ListParams::default())
                .await?
                .into_iter()
                .filter(|parent| parent.validate_zone(&zone))
                .max_by_key(|parent| parent.fqdn().unwrap().len())
            {
                set_zone_parent_ref(ctx.client.clone(), &zone, longest_parent_zone.zone_ref())
                    .await?;
            } else {
                warn!(
                    "zone {} ({}) does not fit into any found parent Zone",
                    zone.name_any(),
                    &zone.spec.domain_name
                );
            };
        }
        (Some(zone_ref), true) => {
            warn!("zone {zone} has both a fully qualified domain_name ({}) and a zoneRef({zone_ref}). It cannot have both.", zone.spec.domain_name);
            return Ok(Action::requeue(Duration::from_secs(300)));
        }
        (None, false) => {
            warn!("{zone} has neither zoneRef nor a fully qualified domainName, making it impossible to deduce its parent zone.");
            return Ok(Action::requeue(Duration::from_secs(300)));
        }
    }

    update_zone_status(zone, ctx.client.clone()).await?;
    Ok(Action::requeue(Duration::from_secs(300)))
}

async fn set_zone_fqdn(client: Client, zone: &Zone, fqdn: &str) -> Result<(), kube::Error> {
    if zone
        .status
        .as_ref()
        .and_then(|status| status.fqdn.as_deref())
        != Some(fqdn)
    {
        info!("updating fqdn for zone {} to {}", zone.name_any(), fqdn);
        Api::<Zone>::namespaced(client, zone.namespace().as_ref().unwrap())
            .patch_status(
                &zone.name_any(),
                &PatchParams::apply(CONTROLLER_NAME),
                &Patch::Merge(json!({
                    "status": {
                        "fqdn": fqdn,
                    }
                })),
            )
            .await?;
    } else {
        debug!("not updating fqdn for zone {} {fqdn}", zone.name_any())
    }
    Ok(())
}

async fn set_zone_parent_ref(
    client: Client,
    zone: &Arc<Zone>,
    parent_ref: ZoneRef,
) -> Result<(), kube::Error> {
    if zone.labels().get(PARENT_ZONE_LABEL) != Some(&parent_ref.as_label()) {
        info!(
            "updating zone {}'s {PARENT_ZONE_LABEL} to {parent_ref}",
            zone.name_any()
        );

        Api::<Zone>::namespaced(client, zone.namespace().as_ref().unwrap())
            .patch_metadata(
                &zone.name_any(),
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
            "not updating zone {}'s {PARENT_ZONE_LABEL} since it is already {parent_ref}",
            zone.name_any()
        )
    }
    Ok(())
}

async fn find_zone_nameserver_records(
    zone: &Zone,
    client: Client,
) -> Result<Vec<ZoneEntry>, kube::Error> {
    // Reference to this zone, which other zones and records will use to refer to it by.
    let zone_ref = ListParams::default().labels(&format!(
        "{PARENT_ZONE_LABEL}={}",
        zone.zone_ref().as_label()
    ));

    let records = Api::<Record>::all(client.clone()).list(&zone_ref).await?;

    // Reflect self-referential NS records from the subzone in the parent zone.
    let mut ns_records: Vec<_> = records
        .iter()
        .map(|record| &record.spec)
        .filter(|spec| spec.class.to_uppercase() == "IN")
        .filter(|spec| spec.type_.to_uppercase() == "NS")
        .filter(|spec| spec.domain_name == "@" || Some(spec.domain_name.as_str()) == zone.fqdn())
        .map(|spec| ZoneEntry {
            fqdn: zone.spec.domain_name.clone(),
            type_: spec.type_.clone(),
            class: spec.class.clone(),
            ttl: spec.ttl.unwrap_or(zone.spec.ttl),
            rdata: spec.rdata.clone(),
        })
        .collect();

    // We also need to copy any A/AAAA records pointed to by the above NS records, to act
    // as glue records. Glue records reside with the parent zone to instruct resolvers where
    // to go for subzone domains. Without it, the resolver would only get back the NS record
    // pointing at a domain like ns1.sub.example.org., but would have no way of resolving that
    // domain itself.
    let glue_records: Vec<_> = records
        .iter()
        .filter(|record| record.spec.class.to_uppercase() == "IN")
        .filter(|record| {
            record.spec.type_.to_uppercase() == "A" || record.spec.type_.to_uppercase() == "AAAA"
        })
        .filter(|record| {
            ns_records
                .iter()
                .any(|ns| Some(ns.rdata.as_str()) == record.fqdn())
        })
        .map(|record| ZoneEntry {
            fqdn: record.fqdn().unwrap().to_string(),
            type_: record.spec.type_.clone(),
            class: record.spec.class.clone(),
            ttl: record.spec.ttl.unwrap_or(zone.spec.ttl),
            rdata: record.spec.rdata.clone(),
        })
        .collect();

    ns_records.extend(glue_records.into_iter());
    Ok(ns_records)
}

async fn update_zone_status(zone: Arc<Zone>, client: Client) -> Result<(), kube::Error> {
    let Some(origin) = zone.fqdn() else {
        return Ok(());
    };

    // Reference to this zone, which other zones and records will use to refer to it by.
    let zone_ref = ListParams::default().labels(&format!(
        "{PARENT_ZONE_LABEL}={}",
        zone.zone_ref().as_label()
    ));

    // Using a VecDeque here because we want to push_front an SOA record
    // after all other records have been hashed.
    let mut entries = VecDeque::new();

    // Insert all child records into the entries list
    for record in Api::<Record>::all(client.clone())
        .list(&zone_ref)
        .await?
        .into_iter()
        .map(|record| record.spec)
    {
        entries.push_back(ZoneEntry {
            fqdn: record.domain_name,
            type_: record.type_,
            class: record.class,
            ttl: record.ttl.unwrap_or(zone.spec.ttl),
            rdata: record.rdata,
        })
    }

    // Create any NS records defined in the child zones, for their own domains.
    // For example, with a top-level domain of `example.org.` and a subdomain of
    // `subdomain.example.org.`, the subdomain might have NS-records like:
    //
    //      @ 360 IN NS ns.subdomain.example.org.
    //
    // Which also need to be represented in the parent zone, so delegation works
    // without having to manually configure NS records in the parent.
    for child_zone in Api::<Zone>::all(client.clone())
        .list(&zone_ref)
        .await?
        .into_iter()
    {
        entries.extend(find_zone_nameserver_records(&child_zone, client.clone()).await?)
    }

    let mut hasher = DefaultHasher::new();
    (&zone.spec, &entries).hash(&mut hasher);
    let new_hash = hasher.finish().to_string();

    let current_hash = zone.status.as_ref().and_then(|status| status.hash.as_ref());

    let last_serial = zone
        .status
        .as_ref()
        .and_then(|status| status.serial)
        .unwrap_or_default();

    // If the hash changed, we need to update the serial.
    let serial = if current_hash != Some(&new_hash) {
        info!("zone {zone}'s hash changed (before: {current_hash:?}, now: {new_hash}), updating serial.");
        // Compute a serial based on the current datetime in UTC as per:
        // https://datatracker.ietf.org/doc/html/rfc1912#section-2.2
        let now = time::OffsetDateTime::now_utc();
        #[rustfmt::skip]
        let now_serial
            = now.year()  as u32 * 1000000
            + now.month() as u32 * 10000
            + now.day()   as u32 * 100;

        // If it's a new day, use YYYYMMDD00, otherwise just use the increment
        // of the old serial.
        std::cmp::max(now_serial, last_serial + 1)
    } else {
        last_serial
    };

    // Insert a SOA record at the beginning of the entry list.
    let ZoneSpec {
        ttl,
        refresh,
        retry,
        expire,
        negative_response_cache,
        ..
    } = zone.spec;

    entries.push_front(ZoneEntry {
        fqdn: origin.to_string(),
        type_: "SOA".to_string(),
        class: "IN".to_string(),
        ttl,
        rdata: format!("ns.{origin} noc.{origin} ({serial} {refresh} {retry} {expire} {negative_response_cache})"),
    });

    Api::<Zone>::namespaced(client, zone.namespace().as_ref().unwrap())
        .patch_status(
            &zone.name_any(),
            &PatchParams::apply(CONTROLLER_NAME),
            &Patch::Merge(json!({
                "status": {
                    "hash": new_hash,
                    "entries": entries,
                    "serial": Some(serial)
                },
            })),
        )
        .await?;

    Ok(())
}

fn zone_error_policy(zone: Arc<Zone>, error: &kube::Error, _ctx: Arc<Data>) -> Action {
    error!(
        "zone {} reconciliation encountered error: {error}",
        zone.name_any()
    );
    Action::requeue(Duration::from_secs(60))
}
