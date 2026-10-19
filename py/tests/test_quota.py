from hedgehog.llm.base import quota_exhausted


class FakeError(Exception):
    def __init__(self, body=None, code=None):
        super().__init__("429")
        self.body = body
        self.code = code
        self.status_code = 429


def test_openai_out_of_credits_is_not_retryable():
    body = {"error": {"type": "insufficient_quota", "code": "credit_balance_exhausted"}}
    assert quota_exhausted(FakeError(body=body))
    assert quota_exhausted(FakeError(code="insufficient_quota"))


def test_plain_rate_limit_is_still_retryable():
    assert not quota_exhausted(FakeError(body={"error": {"type": "requests", "code": "rate_limit_exceeded"}}))
    assert not quota_exhausted(FakeError())
