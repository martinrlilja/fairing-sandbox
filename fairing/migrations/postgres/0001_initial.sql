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
    created_time TIMESTAMPTZ NOT NULL
);

CREATE TABLE team_members (
    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,
    user_id UUID REFERENCES users (id) ON DELETE CASCADE NOT NULL,
    created_time TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (team_id, user_id)
);

-- Global blob storage
CREATE TABLE blobs (
    checksum BYTEA PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,

    storage_id SMALLINT NOT NULL,
    "size" INTEGER NOT NULL,
    size_on_disk INTEGER NOT NULL,

    compression_algorithm SMALLINT,
    compression_level SMALLINT
);

-- File storage
CREATE TABLE files (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,

    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,

    "size" BIGINT NOT NULL,
    is_valid_utf8 BOOLEAN NOT NULL,

    checksum_blake2b BYTEA NOT NULL,
    checksum_sha256 BYTEA NOT NULL,
    checksum_sha1 BYTEA NOT NULL,

    UNIQUE (team_id, checksum_blake2b)
);

CREATE TABLE file_chunks (
    file_id UUID REFERENCES files (id) ON DELETE CASCADE NOT NULL,
    start_byte_offset BIGINT NOT NULL,
    end_byte_offset BIGINT NOT NULL,

    blob_checksum BYTEA REFERENCES blobs (checksum) ON DELETE RESTRICT NOT NULL,

    PRIMARY KEY (file_id, start_byte_offset, end_byte_offset)
);

-- Sites
CREATE TABLE sites (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT UNIQUE NOT NULL,
    team_id UUID REFERENCES teams (id) ON DELETE RESTRICT NOT NULL
);

-- Site sources
CREATE TABLE site_sources (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT NOT NULL,

    site_id UUID REFERENCES sites (id) ON DELETE CASCADE NOT NULL,

    hook_token TEXT NOT NULL,

    last_refresh_time TIMESTAMPTZ DEFAULT NULL,
    --pending_refresh BOOLEAN NOT NULL DEFAULT FALSE,

    UNIQUE (site_id, name)
);

CREATE TABLE site_source_git (
    site_source_id UUID PRIMARY KEY REFERENCES site_sources (id) ON DELETE CASCADE,
    repository_url TEXT NOT NULL,
    id_ed25519_public_key BYTEA,
    id_ed25519_secret_key BYTEA
);

-- Domains
CREATE TABLE domains (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    site_id UUID REFERENCES sites (id) ON DELETE CASCADE NOT NULL,

    fqdn TEXT NOT NULL,
    is_validated BOOLEAN DEFAULT TRUE
);

CREATE UNIQUE INDEX domain_fqdn ON domains (fqdn) WHERE is_validated = TRUE;

-- Storage trees
CREATE TABLE trees (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,

    site_id UUID REFERENCES sites (id) ON DELETE CASCADE NOT NULL,

    reference TEXT NOT NULL,
    version BIGINT NOT NULL,
    draft_version BIGINT NOT NULL,

    parent_tree_id UUID REFERENCES trees (id) ON DELETE CASCADE,
    parent_tree_version BIGINT,

    UNIQUE (site_id, reference)
);

CREATE TABLE tree_leaves (
    tree_id UUID REFERENCES trees (id) ON DELETE CASCADE NOT NULL,
    version BIGINT NOT NULL,

    path TEXT NOT NULL,
    is_deleted BOOLEAN NOT NULL,

    file_id UUID REFERENCES files (id) NOT NULL,

    PRIMARY KEY (tree_id, path, version)
);

-- Deployments
CREATE TABLE deployments (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT UNIQUE NOT NULL,

    site_id UUID REFERENCES sites (id) ON DELETE CASCADE NOT NULL,
    site_source_id UUID REFERENCES site_sources (id) ON DELETE SET NULL,

    status TEXT NOT NULL,
    reference TEXT NOT NULL,

    config JSONB,

    tree_id UUID REFERENCES trees (id) ON DELETE SET NULL,
    tree_version BIGINT NOT NULL,

    commit_id TEXT,
    commit_message TEXT,
    commit_author_name TEXT,
    commit_author_email TEXT,

    UNIQUE (site_id, name)
);

ALTER TABLE sites ADD current_deployment_id UUID REFERENCES deployments (id) ON DELETE SET NULL;
