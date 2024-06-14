pub mod record;
pub mod zone;

use std::{fmt::Debug, sync::Arc};

use json_patch::{PatchOperation, RemoveOperation};
use k8s_openapi::{
    serde::{de::DeserializeOwned, Serialize},
    serde_json::json,
    NamespaceResourceScope,
};
use kube::{
    api::{Patch, PatchParams},
    Api, Client, Resource, ResourceExt,
};
use kubizone_common::FullyQualifiedDomainName;
use kubizone_crds::{
    v1alpha1::{DomainExt, ZoneRef},
    PARENT_ZONE_LABEL,
};
use tracing::{debug, info};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effect {
    None,
    Changed,
}

impl Effect {
    pub fn changed(&self) -> bool {
        self == &Effect::Changed
    }
}

/// Configure the kubi.zone/parent-zone label for the resource.
///
/// If `parent` is none, the label will be deleted instead.
pub async fn set_parent<R>(
    controller_name: &'static str,
    client: Client,
    resource: &Arc<R>,
    parent: Option<ZoneRef>,
) -> Result<Effect, kube::Error>
where
    R: Resource + ResourceExt + Clone + Debug + DeserializeOwned + Serialize,
    R: Resource<Scope = NamespaceResourceScope>,
    <R as Resource>::DynamicType: Default,
{
    match (resource.labels().get(PARENT_ZONE_LABEL), parent) {
        (None, None) => {
            debug!("parent zone already null.");
            Ok(Effect::None)
        }
        (Some(current), Some(desired)) if current == &desired.as_label() => {
            debug!("parent zone already set to {desired}");
            Ok(Effect::None)
        }
        (_, None) => {
            info!(
                "updating {} {}'s {PARENT_ZONE_LABEL}",
                R::kind(&R::DynamicType::default()),
                resource.name_any()
            );
            Api::<R>::namespaced(client, resource.namespace().as_ref().unwrap())
                .patch_metadata(
                    &resource.name_any(),
                    &PatchParams::apply(&controller_name),
                    &Patch::<R>::Json(json_patch::Patch(vec![PatchOperation::Remove(
                        RemoveOperation {
                            path: format!(
                                "/metadata/labels/{}",
                                PARENT_ZONE_LABEL.replace("/", "~1")
                            ),
                        },
                    )])),
                )
                .await?;

            Ok(Effect::Changed)
        }
        (_, Some(desired)) => {
            info!(
                "updating {} {}'s {PARENT_ZONE_LABEL} to {desired}",
                R::kind(&R::DynamicType::default()),
                resource.name_any()
            );
            Api::<R>::namespaced(client, resource.namespace().as_ref().unwrap())
                .patch_metadata(
                    &resource.name_any(),
                    &PatchParams::apply(controller_name),
                    &Patch::Merge(json!({
                        "metadata": {
                            "labels": {
                                PARENT_ZONE_LABEL: desired.as_label()
                            },
                        }
                    })),
                )
                .await?;

            Ok(Effect::Changed)
        }
    }
}

async fn set_fqdn<R>(
    controller_name: &'static str,
    client: Client,
    resource: &Arc<R>,
    fqdn: &FullyQualifiedDomainName,
) -> Result<Effect, kube::Error>
where
    R: Resource + DomainExt + DeserializeOwned,
    R: Resource<Scope = NamespaceResourceScope>,
    <R as Resource>::DynamicType: Default,
{
    if resource.fqdn() == Some(fqdn) {
        debug!(
            "not updating fqdn for {} {} {fqdn}, since it is already set.",
            R::kind(&R::DynamicType::default()),
            resource.name_any()
        );

        return Ok(Effect::None);
    }

    info!(
        "updating fqdn for {} {} to {}",
        R::kind(&R::DynamicType::default()),
        resource.name_any(),
        fqdn
    );
    Api::<R>::namespaced(client, resource.namespace().as_ref().unwrap())
        .patch_status(
            &resource.name_any(),
            &PatchParams::apply(&controller_name),
            &Patch::Merge(json!({
                "status": {
                    "fqdn": fqdn,
                }
            })),
        )
        .await?;

    Ok(Effect::Changed)
}
