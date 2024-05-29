CREATE TABLE IF NOT EXISTS projects (
    id uuid,
    acme_dns_challenge_label text,
    file_encryption_key blob,
    PRIMARY KEY (id)
);

CREATE TABLE IF NOT EXISTS sources (
    project_id uuid,
    bucket bigint,
    name text,

    -- git source settings
    git_repository_url text,
    git_ed25519_secret_key blob,

    PRIMARY KEY ((project_id, bucket), name)
);

CREATE TABLE IF NOT EXISTS layer_sets (
    project_id uuid,
    bucket bigint,
    name text,

    visibility text,

    source_name text,
    source_git_ref text,

    last_layer_id uuid,

    build_current_layer_id uuid,
    build_last_layer_id uuid,

    PRIMARY KEY ((project_id, bucket), name)
);

CREATE TABLE IF NOT EXISTS layers (
    project_id uuid,
    layer_set_name text,
    bucket bigint,
    id uuid,

    status text,

    build_worker_id uuid,
    finalize_worker_id uuid,

    source_git_commit text,

    PRIMARY KEY ((project_id, layer_set_name, bucket), id)
) WITH CLUSTERING ORDER BY (id DESC);

CREATE TABLE IF NOT EXISTS layer_changes (
    project_id uuid,
    layer_set_name text,
    layer_id uuid,
    worker_id uuid,
    bucket bigint,

    path text,
    checksum blob,
    content_encoding_hint bigint,
    headers map<text, text>,

    PRIMARY KEY ((project_id, layer_set_name, layer_id, worker_id, bucket), path)
);

CREATE TABLE IF NOT EXISTS layer_members (
    project_id uuid,
    layer_set_name text,
    layer_id uuid,
    bucket bigint,

    path text,
    checksum blob,
    content_encoding_hint bigint,
    headers map<text, text>,

    PRIMARY KEY ((project_id, layer_set_name, path, bucket), layer_id)
) WITH CLUSTERING ORDER BY (layer_id DESC);

CREATE TABLE IF NOT EXISTS files (
    project_id uuid,
    checksum blob,
    bucket bigint,

    length bigint static,

    offset bigint,
    data blob,

    PRIMARY KEY ((project_id, checksum, bucket), offset)
);

CREATE TABLE IF NOT EXISTS certificates (
    project_id uuid,
    bucket bigint,
    name text,

    domains frozen<list<text>>,

    next_processing_time timestamp,

    keys blob,

    acme_order_url text,
    csr blob,
    csr_secret_key blob,

    PRIMARY KEY ((project_id, bucket), name)
);

CREATE MATERIALIZED VIEW IF NOT EXISTS certificate_queue AS
SELECT
    project_id,
    bucket,
    name,
    next_processing_time
FROM domains
WHERE
    project_id IS NOT NULL
    AND bucket IS NOT NULL
    AND name IS NOT NULL
    AND next_processing_time IS NOT NULL
PRIMARY KEY (next_processing_time, project_id, name, bucket);

CREATE TABLE IF NOT EXISTS validated_domains (
    fqdn text,
    bucket bigint,

    data blob,

    PRIMARY KEY ((fqdn, bucket))
);

CREATE TABLE IF NOT EXISTS acme_challenges (
    acme_dns_challenge_label text,
    project_id uuid,
    certificate_name text,
    dns_01_token text,
    PRIMARY KEY (acme_dns_challenge_label, dns_01_token)
);

-- TODO: delete
CREATE TABLE IF NOT EXISTS domains (
    project_id uuid,
    bucket bigint,
    fqdn text,

    wildcard_kind int,

    last_processing_time timestamp,
    next_processing_time timestamp,

    acme_order_url text,
    certificate_signing_request_status int,

    PRIMARY KEY ((project_id, bucket), fqdn)
);

CREATE MATERIALIZED VIEW IF NOT EXISTS domain_processing AS
SELECT
    project_id,
    bucket,
    fqdn,
    next_processing_time
FROM domains
WHERE
    project_id IS NOT NULL
    AND bucket IS NOT NULL
    AND fqdn IS NOT NULL
    AND next_processing_time IS NOT NULL
PRIMARY KEY (next_processing_time, fqdn);

CREATE TABLE IF NOT EXISTS build_queue_messages (
    bucket bigint,
    id uuid,

    worker_id uuid,

    project_id uuid,
    layer_set_name text,
    layer_id uuid,

    PRIMARY KEY (bucket, id)
);
