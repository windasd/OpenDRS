from __future__ import annotations

import os
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
pytest.importorskip("PySide6.QtCore")
from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from opendrs.contact_import import NewContact, append_contacts  # noqa: E402
from opendrs.f136 import Project  # noqa: E402
from opendrs.gui.filter_bar import TablePage  # noqa: E402
from opendrs.gui.models import make_models  # noqa: E402
from opendrs.records import Contact  # noqa: E402
from opendrs.textcodec import NameCodec  # noqa: E402

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"
NAME, ID = 2, 3


@pytest.fixture(scope="module")
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


def _page(project: Project, key: str = "contacts") -> TablePage:
    model = make_models()[key]
    model.set_project(project, NameCodec.BIG5)
    return TablePage(key, model)


def _column(page: TablePage, col: int) -> list[str]:
    p = page.proxy
    return [p.data(p.index(r, col)) for r in range(p.rowCount())]


def _small_project() -> Project:
    entries = [NewContact(5, "QA0BD", 9990010), NewContact(4, "TG9", 9),
               NewContact(5, "qa0ba", 9990002), NewContact(5, "QA0BD", 9990100),
               NewContact(4, "TG10", 10)]
    project, _ = append_contacts(Project(), entries, NameCodec.ASCII)
    return project


def test_sort_is_numeric_and_reversible(app: QApplication) -> None:
    page = _page(_small_project())
    page.sort_by(ID)
    assert _column(page, ID) == ["9", "10", "9990002", "9990010", "9990100"]
    page.sort_by(ID, Qt.SortOrder.DescendingOrder)
    assert _column(page, ID)[0] == "9990100"
    page.sort_by(NAME)
    assert _column(page, NAME) == ["qa0ba", "QA0BD", "QA0BD", "TG10", "TG9"]
    page.reset()
    assert _column(page, 0) == ["1", "2", "3", "4", "5"]


def test_sorting_never_reorders_the_project(app: QApplication) -> None:
    project = _small_project()
    page = _page(project)
    page.sort_by(ID, Qt.SortOrder.DescendingOrder)
    assert page._model.project is project
    edited: list[Project] = []
    page._model.project_changed.connect(edited.append)
    top = page.proxy.index(0, NAME)  # 9990100 = record #4 in the project
    assert page.proxy.setData(top, "QA0AIL")
    names = [c.name(NameCodec.ASCII) for c in edited[-1].contacts]
    assert names == ["QA0BD", "TG9", "qa0ba", "QA0AIL", "TG10"]


def test_terms_scope_type_and_count(app: QApplication) -> None:
    page = _page(_small_project())
    assert page.count.text() == "共 5 筆"
    page.search.setText("qa0bd 9990")
    assert _column(page, ID) == ["9990010", "9990100"]
    assert page.count.text() == "顯示 2 / 5 筆"
    page.search.setText("10")
    assert page.proxy.rowCount() == 3  # TG10, 9990010, 9990100
    page.scope.setCurrentIndex(page.scope.findText("名稱"))
    assert _column(page, NAME) == ["TG10"]
    page.search.clear()
    page.scope.setCurrentIndex(0)
    assert page.kind is not None
    page.kind.setCurrentIndex(page.kind.findData(Contact.GROUP_CALL))
    assert _column(page, NAME) == ["TG9", "TG10"]
    page.reset()
    assert page.proxy.rowCount() == 5 and page.count.text() == "共 5 筆"


def test_duplicate_filters(app: QApplication) -> None:
    project = _small_project()
    dup = Contact.build(6, 5, project.contacts[2].raw[3:13], 9990002)
    project = Project(contacts=(*project.contacts, dup))
    page = _page(project)
    page.show_box.setCurrentIndex(page.show_box.findText("名稱重複"))
    assert _column(page, ID) == ["9990010", "9990002", "9990100", "9990002"]
    page.show_box.setCurrentIndex(page.show_box.findText("類型與 ID 重複"))
    assert _column(page, 0) == ["3", "6"]


def test_real_project_filters(app: QApplication) -> None:
    from opendrs import f136

    page = _page(f136.load(DH98))
    page.show_box.setCurrentIndex(page.show_box.findText("名稱含換行字元"))
    assert page.proxy.rowCount() == 343
    page.show_box.setCurrentIndex(0)
    assert page.kind is not None
    page.kind.setCurrentIndex(page.kind.findData(Contact.PRIVATE_CALL))
    assert page.proxy.rowCount() == 344
    page.sort_by(ID)
    ids = [int(x) for x in _column(page, ID)]
    assert ids == sorted(ids)


def test_filter_follows_edits(app: QApplication) -> None:
    page = _page(_small_project())
    page.show_box.setCurrentIndex(page.show_box.findText("名稱重複"))
    assert page.proxy.rowCount() == 2
    assert page.proxy.setData(page.proxy.index(0, NAME), "OTHER")
    assert page.proxy.rowCount() == 0


def test_channel_frequency_sorts_numerically(app: QApplication) -> None:
    from opendrs.records import Channel

    chans = tuple(Channel(bytes(32)).with_rx(hz) for hz in (438_800_000, 145_500_000, 1_000_000))
    page = _page(Project(channels=chans), "channels")
    page.sort_by(2)
    assert _column(page, 2) == ["1.00000", "145.50000", "438.80000"]
