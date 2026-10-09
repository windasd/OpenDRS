"""Quick text messages (常用短信): 16 slots of 40 bytes at image offset 0xFAE.

The firmware loads them as one 0x280-byte block from flash 0x3C0FAE (FUN_03052194,
written back by FUN_030522ac); the legacy CPS stores message 1 "CHECK!" at 0xFAE,
NUL padded. Text uses the same code page as names.
"""

from __future__ import annotations

from .textcodec import NameCodec, display_name, encode_name

QUICK_MSG_OFFSET = 0xFAE
QUICK_MSG_SIZE = 40
QUICK_MSG_COUNT = 16
QUICK_MSG_END = QUICK_MSG_OFFSET + QUICK_MSG_SIZE * QUICK_MSG_COUNT


def has_quick_messages(image: bytes) -> bool:
    return len(image) >= QUICK_MSG_END


def quick_messages(image: bytes, codec: NameCodec) -> list[str]:
    if not has_quick_messages(image):
        return []
    return [display_name(image[o:o + QUICK_MSG_SIZE], codec)
            for o in range(QUICK_MSG_OFFSET, QUICK_MSG_END, QUICK_MSG_SIZE)]


def with_quick_message(image: bytes, number: int, text: str, codec: NameCodec) -> bytes:
    """Set message ``number`` (1-16); raises NameEncodingError when it does not fit."""
    if not has_quick_messages(image):
        raise ValueError("設定區太短，沒有常用短信區")
    if not 1 <= number <= QUICK_MSG_COUNT:
        raise ValueError(f"短信編號必須介於 1 與 {QUICK_MSG_COUNT}")
    field = encode_name(text, QUICK_MSG_SIZE, codec)
    offset = QUICK_MSG_OFFSET + (number - 1) * QUICK_MSG_SIZE
    return image[:offset] + field + image[offset + QUICK_MSG_SIZE:]
