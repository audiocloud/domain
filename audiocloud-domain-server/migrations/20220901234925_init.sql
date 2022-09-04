CREATE TABLE IF NOT EXISTS "media"
(
    "app_id"               text NOT NULL,
    "media_id"             text NOT NULL,
    "metadata"             text NULL,
    "path"                 text NULL,
    "download"             text NULL,
    "upload"               text NULL,
    "download_in_progress" int  NOT NULL DEFAULT 0,
    "upload_in_progress"   int  NOT NULL DEFAULT 0,

    PRIMARY KEY ("app_id", "media_id")
);

CREATE INDEX IF NOT EXISTS upload_in_progress_idx ON "media" ("upload_in_progress" ASC);

CREATE INDEX IF NOT EXISTS download_in_progress_idx ON "media" ("download_in_progress" ASC);

CREATE TABLE IF NOT EXISTS "session"
(
    "app_id"     text   NOT NULL,
    "session_id" text   NOT NULL,
    "ends_at"    bigint NOT NULL,

    PRIMARY KEY ("app_id", "session_id")
);

CREATE INDEX IF NOT EXISTS session_ends_at_idx ON "session" ("ends_at" ASC);

CREATE TABLE IF NOT EXISTS "session_media"
(
    "app_id"     text NOT NULL,
    "session_id" text NOT NULL,
    "media_id"   text NOT NULL,

    FOREIGN KEY ("app_id", "session_id") REFERENCES "session" ("app_id", "session_id") on delete cascade,
    FOREIGN KEY ("app_id", "media_id") REFERENCES "media" ("app_id", "media_id") on delete cascade
);
