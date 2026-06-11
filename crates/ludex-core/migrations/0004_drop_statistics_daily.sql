-- Drop `statistics_daily`. Schema v1 created it as a per-day rollup
-- target, but the implemented design computes daily aggregates live
-- from `sessions` (`SessionRepo::daily_playtime_since`) — that stays
-- correct under session deletes and merges with no second bookkeeping
-- path to drift, so the table never gained a writer. Nothing ever
-- inserted into it, so the drop cannot lose data.
--
-- The other dormant v1 tables stay: `forced_applications` and the
-- three emulator tables are claimed by roadmapped features
-- (docs/roadmap.md), and `groups` is referenced by
-- `applications.group_id`.

DROP TABLE statistics_daily;
