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
    team_id UUID REFERENCES teams (id) ON DELETE CASCADE NOT NULL,
    checksum BYTEA NOT NULL,
    "size" BIGINT NOT NULL,

    is_valid_utf8 BOOLEAN NOT NULL,

    PRIMARY KEY (team_id, checksum)
);

CREATE TABLE file_chunks (
    team_id UUID NOT NULL,
    file_checksum BYTEA NOT NULL,
    start_byte_offset BIGINT NOT NULL,
    end_byte_offset BIGINT NOT NULL,

    blob_checksum BYTEA REFERENCES blobs (checksum) ON DELETE RESTRICT NOT NULL,

    PRIMARY KEY (team_id, file_checksum, start_byte_offset, end_byte_offset),
    FOREIGN KEY (team_id, file_checksum) REFERENCES files (team_id, checksum) ON DELETE CASCADE
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
    name TEXT NOT NULL,

    site_source_id UUID REFERENCES site_sources (id) ON DELETE SET NULL,

    version BIGINT NOT NULL,

    UNIQUE (site_source_id, name)
);

CREATE TYPE tree_revision_status AS ENUM (
    'ignore',
    'fetch',
    'fetch_draft',
    'draft',
    'complete'
);

CREATE TABLE tree_revisions (
    tree_id UUID REFERENCES trees (id) ON DELETE CASCADE NOT NULL,
    version BIGINT NOT NULL,
    created_time TIMESTAMPTZ NOT NULL,

    name TEXT NOT NULL,
    status tree_revision_status NOT NULL,

    PRIMARY KEY (tree_id, version),
    UNIQUE (tree_id, name)
);

CREATE TABLE tree_leaves (
    tree_id UUID REFERENCES trees (id) ON DELETE CASCADE NOT NULL,
    path TEXT NOT NULL,
    version BIGINT NOT NULL,

    team_id UUID,
    file_checksum BYTEA,

    PRIMARY KEY (tree_id, path, version),
    FOREIGN KEY (team_id, file_checksum) REFERENCES files (team_id, checksum) ON DELETE CASCADE
);

-- Deployments
CREATE TABLE deployments (
    id UUID PRIMARY KEY,
    created_time TIMESTAMPTZ NOT NULL,
    name TEXT UNIQUE NOT NULL,

    site_id UUID REFERENCES sites (id) ON DELETE CASCADE NOT NULL,

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
