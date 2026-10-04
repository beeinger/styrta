ALTER TABLE conversations
    ADD COLUMN pending_event jsonb;

-- The host is going. Existing meetups had no attendance row for them.
INSERT INTO attendances (event_id, user_id, status)
SELECT e.id, e.host_id, 'going'
FROM events e
WHERE NOT EXISTS (
    SELECT 1
    FROM attendances a
    WHERE a.event_id = e.id AND a.user_id = e.host_id
)
AND (
    e.capacity IS NULL
    OR (
        SELECT count(*)
        FROM attendances a
        WHERE a.event_id = e.id AND a.status = 'going'
    ) < e.capacity
);
