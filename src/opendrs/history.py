"""Undo / redo over immutable :class:`Project` snapshots.

Projects are immutable, so a history is just a list of snapshots — no command
objects, and no way for undo to drift out of sync with what was edited.
"""

from __future__ import annotations

from dataclasses import dataclass

from .f136 import Project

DEFAULT_LIMIT = 100


@dataclass(frozen=True)
class Step:
    project: Project
    label: str


class History:
    def __init__(self, initial: Project, limit: int = DEFAULT_LIMIT) -> None:
        self._limit = max(limit, 1)
        self.reset(initial)

    def reset(self, project: Project) -> None:
        self._past: list[Step] = []
        self._current = Step(project, "")
        self._future: list[Step] = []

    @property
    def current(self) -> Project:
        return self._current.project

    def push(self, project: Project, label: str) -> bool:
        """Record ``project`` as the new state; ignored if nothing changed."""
        if project == self._current.project:
            return False
        self._past.append(self._current)
        if len(self._past) > self._limit:
            self._past.pop(0)
        self._current = Step(project, label)
        self._future.clear()
        return True

    @property
    def can_undo(self) -> bool:
        return bool(self._past)

    @property
    def can_redo(self) -> bool:
        return bool(self._future)

    @property
    def undo_label(self) -> str:
        return self._current.label if self._past else ""

    @property
    def redo_label(self) -> str:
        return self._future[-1].label if self._future else ""

    def undo(self) -> Project | None:
        if not self._past:
            return None
        self._future.append(self._current)
        self._current = self._past.pop()
        return self._current.project

    def redo(self) -> Project | None:
        if not self._future:
            return None
        self._past.append(self._current)
        self._current = self._future.pop()
        return self._current.project
