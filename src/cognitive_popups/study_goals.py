"""Persisted intention periods, independent of buffer and three-hour step sessions."""
import time
import uuid

GAP_SECONDS = 60 * 60


class StudyGoals:
    def __init__(self, records):
        self.records = records
    def current(self):
        rows = self.records._read('SELECT * FROM study_goal_periods ORDER BY accepted_epoch DESC, rowid DESC LIMIT 1', ())
        return rows[0] if rows else None

    def needs_goal(self, *, now=None, active_step=False):
        period = self.current()
        if active_step:
            return False
        return not period or (time.time() if now is None else now) - period['activity_epoch'] >= GAP_SECONDS

    def accept(self, text, *, material='', origin='', step_session_key='', now=None):
        # Each explicit renewal has its own intention: old steps retain their goal.
        moment = time.time() if now is None else now
        self.records.save_intention(text, source='study', material=material, material_origin=origin,
                                    study_period=(uuid.uuid4().hex, step_session_key, moment))
        return self.current()

    def touch(self, *, now=None):
        period = self.current()
        moment = time.time() if now is None else now
        if period:
            # Once due, renewal stays due even if an active step postpones its
            # form. A late completion or another tool cannot revive an old goal.
            self.records._write('UPDATE study_goal_periods SET activity_epoch=max(activity_epoch, ?) WHERE id=? AND activity_epoch>?',
                                (moment, period['id'], moment-GAP_SECONDS), 'study activity')

    def bind_step_session(self, key):
        period = self.current()
        if period:
            self.records._write('UPDATE study_goal_periods SET step_session_key=? WHERE id=?',
                                (key, period['id']), 'study goal step binding')
