# Integration Tests

These tests deploy:

    * A development version of the Kubizone Custom Resource Definitions (dev.kubi.zone)
    * Zone and Record resources using these development CRDs
    * The Kubizone Operator, but operating only on these development CRDs.

This is to enable users to perform Kubizone development on clusters already running production versions of Kubizone, without interfering.


## Tests

### record_adoption

Creates a Zone and a Record with an FQDN matching that of the zone, ensuring that the kubizone operator correctly forces adoption of the record.

### orphaned_record

As above, but then deletes the parent zone, and verifies that the record's parent zone label is removed.

### record_delegation_withdrawn

Ensures parent-zone labels are removed from records, if the already determined parent zone's delegations no longer matches this record.

### cross_namespace_adoption

Creates zone `example.org.` in the namespace `cross-namespace-adoption` allowing delegation to all records in `default` namespace. Subsequently, it creates a `good.example.org.` record in `default` namespace and a `bad.example.org.` record in the `cross_namespace_adoption` namespace, and verifies that only the `good.example.org.` record is adoptd.

### split_namespace_delegation

Creates: 
* Zone `example.org` in namespace `split-namespace-delegation` which delegates `*.example.org` and `*.dev.example.org` to `kubizone-split-prod` and `kubizone-split-dev` respectively.
* Record `good.example.org` in `kubizone-split-prod` namespace. Verifies adoption.
* Record `good.dev.example.org` in `kubizone-split-dev` namespace. Verifies adoption.
* Record `bad.example.org` in `kubizone-split-dev` namespace. Verifies that this record is *not* adopted.

### limited_adoption

Creates:
* Zone `example.org.` with record delegation for `A` record `good.*`.
* `A`-record `good.example.org.`. Verifies adoption.
* `A`-record `bad.example.org.`. Verifies non-adoption due to `bad` not being delegated.
* `AAAA`-record `good.example.org`. Verifies non-adoption due to `AAAA` not being delegated.

### longest_matching
Creates:
* Zone `example.org` with no delegation rules.
* Zone `sub.example.org` with record delegation to `*`.
* Record `good.sub.sub.example.org`. Verifies that record is adopted by `sub.example.org.` and *not* `example.org`.