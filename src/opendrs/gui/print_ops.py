"""MainWindow mixin: 列印／預覽列印／匯出 PDF (the report in print_report)."""

from __future__ import annotations

from pathlib import Path

from PySide6.QtPrintSupport import QPrintDialog, QPrinter, QPrintPreviewDialog
from PySide6.QtWidgets import QDialog, QFileDialog, QMessageBox

from .print_report import document, report_html, setup_printer, write_pdf

PDF_FILTER = "PDF (*.pdf)"


class PrintOpsMixin:
    def _report_title(self) -> str:
        return self._path.stem if self._path else (self._source or "寫頻資料")

    def report_html(self) -> str:
        return report_html(self._project, self.codec, self._report_title())

    def print_project(self) -> bool:
        if not self._require_project("列印"):
            return False
        printer = setup_printer(QPrinter(QPrinter.PrinterMode.HighResolution))
        dialog = QPrintDialog(printer, self)
        dialog.setWindowTitle("列印")
        if dialog.exec() != QDialog.DialogCode.Accepted:
            return False
        document(self.report_html()).print_(printer)
        return True

    def print_preview(self) -> QPrintPreviewDialog | None:
        if not self._require_project("預覽列印"):
            return None
        doc = document(self.report_html())
        printer = setup_printer(QPrinter(QPrinter.PrinterMode.HighResolution))
        preview = QPrintPreviewDialog(printer, self)
        doc.setParent(preview)
        # the dialog does not own the printer; a collected QPrinter crashes it
        self._preview = (preview, printer)
        preview.setWindowTitle("預覽列印")
        preview.paintRequested.connect(doc.print_)
        preview.resize(1100, 800)
        preview.open()
        return preview

    def export_pdf(self, path: Path | None = None) -> bool:
        if not self._require_project("匯出 PDF"):
            return False
        if path is None:
            default = (self._path.with_suffix(".pdf") if self._path
                       else Path(self._settings.last_dir or ".") / "寫頻資料.pdf")
            chosen, _ = QFileDialog.getSaveFileName(self, "匯出 PDF", str(default), PDF_FILTER)
            if not chosen:
                return False
            path = Path(chosen)
        try:
            write_pdf(self.report_html(), path)
        except OSError as exc:
            QMessageBox.critical(self, "匯出失敗", str(exc))
            return False
        self.statusBar().showMessage(f"已匯出 PDF 到 {path}", 8000)
        return True
