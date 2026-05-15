import argparse
from unittest.mock import MagicMock, patch


def test_cmd_list_without_days_does_not_print_none(capsys):
    """When --days is not given but a date filter is active, the heading
    must not contain the literal string 'None'."""
    from agent_recall.cli import cmd_list

    mock_conv = {
        "last_message_at": "2026-01-01T10:00:00Z",
        "conversation_summary": "Test conversation",
        "message_count": 3,
        "project_path": "home-user-project",
        "session_id": "test-session",
        "source": "claude",
    }

    args = argparse.Namespace(
        days=None,
        since="2026-01-01",
        until=None,
        date=None,
        source=None,
        limit=5,
        json=False,
        no_index=True,
    )

    with patch("agent_recall.cli.ConversationSearch") as MockCS:
        mock_search = MagicMock()
        mock_search.list_recent_conversations.return_value = [mock_conv]
        MockCS.return_value = mock_search
        cmd_list(args)

    captured = capsys.readouterr()
    assert "None" not in captured.out
