Kubernetes ecosystem of DNS resources and controllers.

The core of kubi.zone consists of the Kubernetes Custom Resources Record and Zone, as well as the kubizone controller, which manages their relations after creation.

See https://kubi.zone for in-depth information on deploying, running and using Kubizone

## Data Flow

The Kubizone controller keeps track of Records and Zones within the Kubernetes cluster, and uses this information to populate the `.status.entries` field of a Zone with the adopted records which have been vetted according to the delegations specified in the Zone.

Then, the provider-specific controller can use this populates `.status.entries` field to push DNS changes as it sees fit.

Here's a diagram showing how data is read and written for each of the respective controllers:

```mermaid
flowchart LR
    subgraph Zone
        zone-spec["
        spec:
        #nbsp;#nbsp;#nbsp;#nbsp;domainName: kubi.zone.
        #nbsp;#nbsp;#nbsp;#nbsp;delegations:
        #nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;- records:
        #nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;- pattern: #quot;*#quot;
        "]
        style zone-spec text-align:left

        zone-status["
        status:
        #nbsp;#nbsp;#nbsp;#nbsp;entries:
        #nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;- fqdn: www.kubi.zone.
        #nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;type: A
        #nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;#nbsp;rdata: 127.0.0.1
        "]
        style zone-status text-align:left
    end

    subgraph Record
        record-spec["
        spec:
        #nbsp;#nbsp;#nbsp;#nbsp;domainName: www.kubi.zone.
        #nbsp;#nbsp;#nbsp;#nbsp;type: A
        #nbsp;#nbsp;#nbsp;#nbsp;rdata: 127.0.0.1
        "]
        style record-spec text-align:left
    end

    kubizone-controller --reads-->record-spec
    kubizone-controller --reads-->zone-spec
    kubizone-controller --writes-->zone-status

    cloudflare-integration --reads-->zone-status
    cloudflare-integration --writes-->CloudFlareZone["CloudFlare Zone"]

```

And here's a more in-depth sequence diagram:


```mermaid
sequenceDiagram
    actor User
    create participant Kubernetes
    note left of Kubernetes: Allows delegation of all<br/> *.kubi.zone domains
    User->>Kubernetes: Create Zone kubi.zone.
    User->>Kubernetes: Create Record www.kubi.zone.

    loop Reconciliation Loop
        create participant kubizone-controller
        Kubernetes->>kubizone-controller: Controller reads Zones and Records
        note left of kubizone-controller: Checks Record against Zone delegation rules
        destroy kubizone-controller
        kubizone-controller->>Kubernetes: Updates .status.entries field of Zone.
    end

    loop Reconciliation Loop
        create participant cloudflare-integration
        Kubernetes->>cloudflare-integration: Reads state
        note left of cloudflare-integration: Checks Record against Zone delegation rules
        destroy cloudflare-integration
        cloudflare-integration->>Kubernetes: Updates .status.entries field of Zone.
    end

```