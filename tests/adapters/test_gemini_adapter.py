import json
from pathlib import Path
from conversation_search.adapters.gemini import GeminiAdapter


def test_parse_basic_conversation(tmp_path):
    chat_file = tmp_path / "chat-20240201-100000.json"
    chat_file.write_text(json.dumps([
        {
            "role": "user",
            "parts": [{"text": "What is the capital of France?"}],
            "timestamp": "2024-02-01T10:00:00Z",
        },
        {
            "role": "model",
            "parts": [{"text": "Paris."}],
            "timestamp": "2024-02-01T10:00:01Z",
        },
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)

    assert meta.source == "gemini"
    assert len(messages) == 2
    assert messages[0].role == "user"
    assert messages[0].content == "What is the capital of France?"
    assert messages[0].source == "gemini"
    assert messages[1].role == "ai"  # normalised from "model"
    assert "Paris" in messages[1].content


def test_parse_concatenates_multi_part(tmp_path):
    chat_file = tmp_path / "chat-multi.json"
    chat_file.write_text(json.dumps([
        {
            "role": "user",
            "parts": [{"text": "Part one."}, {"text": "Part two."}],
            "timestamp": "2024-02-01T11:00:00Z",
        },
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert "Part one." in messages[0].content
    assert "Part two." in messages[0].content


def test_parse_tolerates_missing_timestamp(tmp_path):
    chat_file = tmp_path / "chat-no-ts.json"
    chat_file.write_text(json.dumps([
        {"role": "user", "parts": [{"text": "Hello"}]},
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert len(messages) == 1
    assert messages[0].timestamp == ""


def test_parse_skips_unknown_roles(tmp_path):
    chat_file = tmp_path / "chat-roles.json"
    chat_file.write_text(json.dumps([
        {"role": "user", "parts": [{"text": "Hi"}], "timestamp": "2024-02-01T12:00:00Z"},
        {"role": "system", "parts": [{"text": "System prompt"}], "timestamp": "2024-02-01T12:00:00Z"},
        {"role": "model", "parts": [{"text": "Hello!"}], "timestamp": "2024-02-01T12:00:01Z"},
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert len(messages) == 2
    assert messages[0].role == "user"
    assert messages[1].role == "ai"


def test_parse_tolerates_invalid_json(tmp_path):
    chat_file = tmp_path / "chat-bad.json"
    chat_file.write_text("this is not json")
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert messages == []


def test_parse_tolerates_non_list_top_level(tmp_path):
    chat_file = tmp_path / "chat-obj.json"
    chat_file.write_text(json.dumps({"role": "user", "parts": []}))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)
    assert messages == []


def test_scan_finds_chat_files(tmp_path, monkeypatch):
    chats_dir = tmp_path / ".gemini" / "tmp" / "proj123" / "chats"
    chats_dir.mkdir(parents=True)
    chat = chats_dir / "chat-20240201-100000.json"
    chat.write_text("[]")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    adapter = GeminiAdapter()
    paths = adapter.scan(days_back=None)
    assert chat in paths


def test_scan_excludes_non_chats_json(tmp_path, monkeypatch):
    gemini_dir = tmp_path / ".gemini" / "tmp"
    gemini_dir.mkdir(parents=True)
    # A JSON file NOT in a chats/ subdirectory should be excluded
    other = gemini_dir / "settings.json"
    other.write_text("{}")

    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    adapter = GeminiAdapter()
    paths = adapter.scan(days_back=None)
    assert other not in paths


def test_message_uuids_and_parent_chain(tmp_path):
    chat_file = tmp_path / "mysession.json"
    chat_file.write_text(json.dumps([
        {"role": "user", "parts": [{"text": "A"}], "timestamp": "2024-01-01T00:00:00Z"},
        {"role": "model", "parts": [{"text": "B"}], "timestamp": "2024-01-01T00:00:01Z"},
        {"role": "user", "parts": [{"text": "C"}], "timestamp": "2024-01-01T00:00:02Z"},
    ]))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)

    assert messages[0].uuid == "mysession-0"
    assert messages[0].parent_uuid is None
    assert messages[1].uuid == "mysession-1"
    assert messages[1].parent_uuid == "mysession-0"
    assert messages[2].uuid == "mysession-2"
    assert messages[2].parent_uuid == "mysession-1"


def test_parse_modern_dict_format(tmp_path):
    chat_file = tmp_path / "modern.json"
    chat_file.write_text(json.dumps({
        "sessionId": "modern-session-123",
        "messages": [
            {
                "id": "msg-1",
                "type": "user",
                "content": [{"text": "Hello"}]
            },
            {
                "id": "msg-2",
                "type": "gemini",
                "content": [{"text": "Hi there"}]
            }
        ]
    }))
    adapter = GeminiAdapter()
    meta, messages = adapter.parse(chat_file)

    assert meta.session_id == "modern-session-123"
    assert len(messages) == 2
    assert messages[0].uuid == "msg-1"
    assert messages[0].role == "user"
    assert messages[0].content == "Hello"
    assert messages[1].uuid == "msg-2"
    assert messages[1].role == "ai"
    assert messages[1].content == "Hi there"
