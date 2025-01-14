-- Users
CREATE TABLE users (
    id UUID PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    password TEXT NOT NULL,
    created_time TIMESTAMPTZ NOT NULL
);

-- Teams
CREATE TABLE teams (
    id UUID PRIMARY KEY,
    name TEXT UNIQUE NOT NULL,
    created_time TIMESTAMPTZ NOT NULL,
    file_keyspace_id UUID NOT NULL
);

CREATE TABLE team_members (
    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,
    user_id UUID REFERENCES users (id) ON DELETE CASCADE NOT NULL,
    created_time TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (team_id, user_id)
);

-- Sources
CREATE TABLE sources (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT NOT NULL,

    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,

    hook_token TEXT NOT NULL,

    last_refresh_time TIMESTAMPTZ DEFAULT NULL,

    UNIQUE (team_id, name)
);

CREATE TABLE source_git (
    source_id UUID PRIMARY KEY REFERENCES sources (id) ON DELETE CASCADE,
    repository_url TEXT NOT NULL,
    id_ed25519_secret_key BYTEA
);

-- Sites
CREATE TABLE sites (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT UNIQUE NOT NULL,
    team_id UUID REFERENCES teams (id) ON DELETE RESTRICT NOT NULL,
    base_source_id UUID REFERENCES sources (id) ON DELETE RESTRICT NOT NULL
);

-- Storage layers
CREATE TABLE layer_sets (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT NOT NULL,

    source_id UUID REFERENCES sources (id) ON DELETE SET NULL,

    UNIQUE (source_id, name)
);

-- Builds
CREATE TYPE build_status AS ENUM (
    'queued',
    'building',
    'complete'
);

CREATE TABLE builds (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT NOT NULL UNIQUE,

    layer_set_id UUID REFERENCES layer_sets (id) ON DELETE SET NULL,
    layer_id UUID NOT NULL,

    status build_status NOT NULL,

    source_reference TEXT NOT NULL
);

-- Deployments
CREATE TABLE deployments (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT NOT NULL,

    site_id UUID REFERENCES sites (id) ON DELETE CASCADE NOT NULL,

    UNIQUE (site_id, name)
);

ALTER TABLE sites ADD current_deployment_id UUID REFERENCES deployments (id) ON DELETE SET NULL;

-- Projections
CREATE TABLE deployment_projections (
    id UUID PRIMARY KEY,

    deployment_id UUID REFERENCES deployments (id) ON DELETE CASCADE NOT NULL,
    layer_set_id UUID REFERENCES layer_sets (id) ON DELETE CASCADE NOT NULL,
    layer_id UUID NOT NULL,

    mount_path TEXT NOT NULL,
    sub_path TEXT NOT NULL
);

-- File storage
CREATE TABLE file_keyspace (
    id UUID PRIMARY KEY,
    key BYTEA NOT NULL
);

CREATE TABLE blobs (
    checksum BYTEA PRIMARY KEY,

    storage_id SMALLINT NOT NULL,
    "size" INTEGER NOT NULL,
    size_on_disk INTEGER NOT NULL,

    compression_algorithm SMALLINT,
    compression_level SMALLINT
);

CREATE TABLE files (
    file_keyspace UUID REFERENCES file_keyspace (id) ON DELETE CASCADE NOT NULL,
    checksum BYTEA NOT NULL,
    "size" BIGINT NOT NULL,

    is_valid_utf8 BOOLEAN NOT NULL,

    PRIMARY KEY (file_keyspace, checksum)
);

CREATE TABLE file_chunks (
    file_keyspace UUID NOT NULL,
    file_checksum BYTEA NOT NULL,
    start_byte_offset BIGINT NOT NULL,
    end_byte_offset BIGINT NOT NULL,

    blob_checksum BYTEA REFERENCES blobs (checksum) ON DELETE RESTRICT NOT NULL,

    PRIMARY KEY (file_keyspace, file_checksum, start_byte_offset, end_byte_offset),
    FOREIGN KEY (file_keyspace, file_checksum) REFERENCES files (file_keyspace, checksum) ON DELETE CASCADE
);

CREATE TABLE layer_members (
    layer_set_id UUID NOT NULL,
    layer_id UUID NOT NULL,
    path TEXT NOT NULL,

    file_keyspace UUID,
    file_checksum BYTEA,

    PRIMARY KEY (layer_set_id, path, layer_id),
    FOREIGN KEY (file_keyspace, file_checksum) REFERENCES files (file_keyspace, checksum) ON DELETE CASCADE
);

-- Domains
CREATE TABLE domains (
    id UUID NOT NULL,
    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,
    site_id UUID REFERENCES sites (id) ON DELETE CASCADE DEFAULT NULL,

    created_time TIMESTAMPTZ NOT NULL,

    name TEXT NOT NULL,
    acme_label TEXT UNIQUE NOT NULL,
    is_validated BOOLEAN DEFAULT FALSE,

    PRIMARY KEY (team_id, id)
);

CREATE UNIQUE INDEX domain_teams_name ON domains (team_id, name);
CREATE UNIQUE INDEX domain_global_name ON domains (name) WHERE is_validated = TRUE;

CREATE TABLE certificates (
    id UUID NOT NULL,
    team_id UUID NOT NULL,
    domain_id UUID NOT NULL,

    created_time TIMESTAMPTZ NOT NULL,
    expires_time TIMESTAMPTZ NOT NULL,

    private_key BYTEA NOT NULL,
    public_key_chain BYTEA[] NOT NULL,

    PRIMARY KEY (team_id, domain_id, id),
    FOREIGN KEY (team_id, domain_id) REFERENCES domains (team_id, id) ON DELETE CASCADE
);

-- ACME
CREATE TYPE acme_order_status AS ENUM (
    'pending',
    'ready',
    'processing',
    'valid',
    'invalid'
);

CREATE TABLE acme_orders (
    id UUID NOT NULL,
    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,

    created_time TIMESTAMPTZ NOT NULL,

    status acme_order_status NOT NULL,
    url TEXT NOT NULL,

    PRIMARY KEY (team_id, id)
);

CREATE TABLE acme_challenges (
    id UUID NOT NULL,
    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,
    acme_order_id UUID NOT NULL,
    domain_id UUID NOT NULL,

    dns_01_token TEXT NOT NULL,

    PRIMARY KEY (team_id, acme_order_id, id),
    FOREIGN KEY (team_id, acme_order_id) REFERENCES acme_orders (team_id, id) ON DELETE CASCADE,
    FOREIGN KEY (team_id, domain_id) REFERENCES domains (team_id, id) ON DELETE CASCADE
);
