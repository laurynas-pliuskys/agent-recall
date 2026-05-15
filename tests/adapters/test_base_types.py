from agent_recall.adapters.base import ParsedMessage, ConversationMeta, BaseAdapter

def test_parsed_message_defaults():
    msg = ParsedMessage(
        uuid="abc123",
        parent_uuid=None,
        session_id="sess1",
        timestamp="2024-01-01T00:00:00Z",
        role="user",
        content="hello",
        source="claude",
    )
    assert msg.is_sidechain is False
    assert msg.project_path == ""

def test_conversation_meta():
    meta = ConversationMeta(
        session_id="sess1",
        source="claude",
        project_path="myproject",
        conversation_file="/path/to/file.jsonl",
        summary="A test conversation",
        leaf_uuid="leaf1",
    )
    assert meta.session_id == "sess1"
