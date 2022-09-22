--- task

DEFINE TABLE task SCHEMAFULL;
DEFINE FIELD from ON task TYPE datetime ASSERT $value > time::now() -1y;
DEFINE FIELD to ON task TYPE datetime ASSERT $value >= from + 1s;
DEFINE FIELD is_active ON task TYPE bool VALUE <future> { from <= time::now() && time::now() <= to };
DEFINE FIELD spec ON task TYPE object;
DEFINE FIELD permissions ON task TYPE object;
DEFINE FIELD fixed_instance_pool ON task TYPE array;
DEFINE FIELD media_pool ON task TYPE array;
DEFINE FIELD revision ON media TYPE int VALUE 0;
DEFINE INDEX idx_from_to ON task COLUMNS from, to;

--- media

DEFINE TABLE media SCHEMAFULL;
DEFINE FIELD app_id ON media TYPE string ASSERT $value != null;
DEFINE FIELD path ON media TYPE string VALUE null;
DEFINE FIELD metadata ON media TYPE object VALUE null;
DEFINE FIELD download ON media TYPE object VALUE null;
DEFINE FIELD upload ON media TYPE object VALUE null;
DEFINE FIELD sessions ON media TYPE array VALUE [];
DEFINE FIELD revision ON media TYPE int VALUE 0;

--- instance

DEFINE TABLE fixed_instance SCHEMAFULLL;
DEFINE FIELD name ON fixed_instance TYPE string ASSERT $value != null;
DEFINE FIELD desired_play_state ON fixed_instance TYPE object VALUE null;
DEFINE FIELD play_state ON fixed_instance TYPE object VALUE null;
DEFINE FIELD desired_power_state ON fixed_instance TYPE object VALUE null;
DEFINE FIELD power_state ON fixed_instance TYPE object VALUE null;
DEFINE FIELD revision ON media TYPE int VALUE 0;

