from __future__ import annotations

from dataclasses import replace

from opendrs.f136 import Project
from opendrs.history import History


def p(n: int) -> Project:
    return replace(Project(), image=bytes([n]))


def test_push_undo_redo_cycle() -> None:
    h = History(p(0))
    assert not h.can_undo and not h.can_redo and h.undo() is None and h.redo() is None
    assert h.push(p(1), "一")
    assert h.push(p(2), "二")
    assert h.undo_label == "二"
    assert h.undo() == p(1) and h.redo_label == "二"
    assert h.undo() == p(0) and not h.can_undo
    assert h.redo() == p(1) and h.redo() == p(2) and not h.can_redo
    assert h.current == p(2)


def test_identical_push_is_ignored_and_new_push_clears_redo() -> None:
    h = History(p(0))
    assert not h.push(p(0), "無變化")
    h.push(p(1), "a")
    h.undo()
    h.push(p(5), "b")
    assert not h.can_redo and h.current == p(5)


def test_limit_and_reset() -> None:
    h = History(p(0), limit=3)
    for i in range(1, 6):
        h.push(p(i), str(i))
    undone = 0
    while h.undo() is not None:
        undone += 1
    assert undone == 3 and h.current == p(2)
    h.reset(p(9))
    assert h.current == p(9) and not h.can_undo and not h.can_redo and h.undo_label == ""
