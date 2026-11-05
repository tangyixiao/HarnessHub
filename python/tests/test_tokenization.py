"""``harness_hub_runtime.tokenization`` 契约层单元测试（S2，TDD RED）。

运行方式（worktree 根目录）::

    python -m unittest discover -s python/tests -p test_tokenization.py -t python -v

契约来源：docs/specs/2026-11-05-f21-provider-gateway-design.md §3.7 / §4.4 / §5 / §8 / §9。
零第三方依赖：测试仅使用 unittest 与 dataclasses 标准库。
"""

from __future__ import annotations

import unittest
from dataclasses import FrozenInstanceError

from harness_hub_runtime.tokenization import (
    LOCAL_LEVELS,
    TOKENIZER_REVISIONS,
    TRUTH_PRECEDENCE,
    RevisionBinding,
    TokenEstimate,
    bind_revision,
    estimate,
    select_truth,
    truth_rank,
)


class ConstantsTests(unittest.TestCase):
    """§5.1 冻结常量。"""

    def test_truth_precedence_exact(self) -> None:
        self.assertEqual(
            TRUTH_PRECEDENCE,
            ("provider_reported", "provider_count_api", "local_exact", "estimated"),
        )
        self.assertIsInstance(TRUTH_PRECEDENCE, tuple)
        self.assertEqual(len(TRUTH_PRECEDENCE), 4)

    def test_local_levels_exact(self) -> None:
        self.assertEqual(LOCAL_LEVELS, ("local_exact", "estimated"))
        self.assertIsInstance(LOCAL_LEVELS, tuple)
        self.assertEqual(len(LOCAL_LEVELS), 2)

    def test_tokenizer_revisions_exact(self) -> None:
        self.assertEqual(
            TOKENIZER_REVISIONS,
            {
                "gpt-4o": ("o200k_base", "2024-05-13"),
                "gpt-4": ("cl100k_base", "2023-06-13"),
                "gpt-3.5": ("cl100k_base", "2023-06-13"),
                "claude-": ("anthropic-claude", "2024-06-20"),
                "gemini-": ("sentencepiece-gemini", "2024-09-24"),
            },
        )
        self.assertEqual(len(TOKENIZER_REVISIONS), 5)


class TruthTests(unittest.TestCase):
    """§5.1 truth_rank / select_truth。"""

    def test_truth_rank_known_levels(self) -> None:
        self.assertEqual(truth_rank("provider_reported"), 0)
        self.assertEqual(truth_rank("provider_count_api"), 1)
        self.assertEqual(truth_rank("local_exact"), 2)
        self.assertEqual(truth_rank("estimated"), 3)

    def test_truth_rank_unknown_is_worst(self) -> None:
        self.assertEqual(truth_rank("bogus"), len(TRUTH_PRECEDENCE))
        self.assertEqual(truth_rank("bogus"), 4)
        self.assertEqual(truth_rank(""), len(TRUTH_PRECEDENCE))

    def test_select_truth_example_local_exact_wins(self) -> None:
        self.assertEqual(
            select_truth({"local_exact": 7, "estimated": 9}), ("local_exact", 7)
        )

    def test_select_truth_example_only_estimated(self) -> None:
        self.assertEqual(select_truth({"estimated": 9}), ("estimated", 9))

    def test_select_truth_example_provider_reported_wins(self) -> None:
        self.assertEqual(
            select_truth({"provider_reported": 12, "local_exact": 11}),
            ("provider_reported", 12),
        )

    def test_select_truth_example_empty(self) -> None:
        self.assertEqual(select_truth({}), ("estimated", None))

    def test_select_truth_first_highest_wins_with_two_candidates(self) -> None:
        self.assertEqual(
            select_truth({"provider_count_api": 5, "estimated": 9}),
            ("provider_count_api", 5),
        )
        self.assertEqual(
            select_truth(
                {
                    "provider_reported": 12,
                    "provider_count_api": 10,
                    "local_exact": 11,
                    "estimated": 13,
                }
            ),
            ("provider_reported", 12),
        )

    def test_select_truth_bool_rejected(self) -> None:
        # bool 是 int 的子类，但必须被拒。
        self.assertEqual(select_truth({"provider_reported": True}), ("estimated", None))
        self.assertEqual(select_truth({"estimated": False}), ("estimated", None))
        self.assertEqual(
            select_truth({"local_exact": True, "estimated": 9}), ("estimated", 9)
        )

    def test_select_truth_negative_rejected(self) -> None:
        self.assertEqual(
            select_truth({"provider_reported": -1, "local_exact": 5}),
            ("local_exact", 5),
        )
        self.assertEqual(select_truth({"estimated": -3}), ("estimated", None))

    def test_select_truth_str_rejected(self) -> None:
        self.assertEqual(select_truth({"provider_reported": "12"}), ("estimated", None))
        self.assertEqual(
            select_truth({"local_exact": "7", "estimated": 9}), ("estimated", 9)
        )

    def test_select_truth_none_ignored(self) -> None:
        self.assertEqual(
            select_truth({"provider_reported": None, "estimated": 9}),
            ("estimated", 9),
        )
        self.assertEqual(select_truth({"local_exact": None}), ("estimated", None))

    def test_select_truth_all_invalid(self) -> None:
        self.assertEqual(
            select_truth(
                {
                    "provider_reported": True,
                    "provider_count_api": -1,
                    "local_exact": "x",
                    "estimated": None,
                }
            ),
            ("estimated", None),
        )

    def test_select_truth_zero_is_valid(self) -> None:
        self.assertEqual(
            select_truth({"provider_reported": 0, "local_exact": 5}),
            ("provider_reported", 0),
        )
        self.assertEqual(select_truth({"estimated": 0}), ("estimated", 0))

    def test_select_truth_unknown_keys_ignored(self) -> None:
        self.assertEqual(
            select_truth({"provider_reported": "bad", "mystery": 99}),
            ("estimated", None),
        )


class EstimateTests(unittest.TestCase):
    """§5.2 estimate heuristic + frozen dataclass + to_json。"""

    def test_estimate_lengths(self) -> None:
        # max(1, ceil(text_length / 4))
        cases = {0: 1, 1: 1, 4: 1, 5: 2, 120: 30}
        for length, expected_n in cases.items():
            with self.subTest(length=length):
                result = estimate(length, "gpt-4o")
                self.assertEqual(result.input_tokens, expected_n)
                self.assertEqual(result.output_tokens, 0)
                self.assertEqual(result.total_tokens, expected_n)
                self.assertEqual(result.total_tokens, result.input_tokens)

    def test_estimate_always_estimated(self) -> None:
        for length in (0, 1, 4, 5, 120):
            with self.subTest(length=length):
                result = estimate(length, "gpt-4o")
                self.assertEqual(result.truth_level, "estimated")
                self.assertIs(result.estimated, True)

    def test_estimate_to_json_exact_keys_and_order(self) -> None:
        result = estimate(120, "gpt-4o")
        self.assertEqual(
            list(result.to_json().keys()),
            [
                "inputTokens",
                "outputTokens",
                "totalTokens",
                "truthLevel",
                "tokenizerModel",
                "revision",
                "estimated",
            ],
        )
        self.assertEqual(
            result.to_json(),
            {
                "inputTokens": 30,
                "outputTokens": 0,
                "totalTokens": 30,
                "truthLevel": "estimated",
                "tokenizerModel": "o200k_base",
                "revision": "2024-05-13",
                "estimated": True,
            },
        )

    def test_estimate_revision_from_bind_revision_gpt4o(self) -> None:
        result = estimate(120, "gpt-4o")
        binding = bind_revision("gpt-4o")
        self.assertEqual(result.tokenizer_model, binding.tokenizer_model)
        self.assertEqual(result.revision, binding.revision)
        self.assertEqual(result.tokenizer_model, "o200k_base")
        self.assertEqual(result.revision, "2024-05-13")

    def test_estimate_invalid_text_length_raises_value_error(self) -> None:
        # -1（负数）/ 1.5（float）/ "10"（str）/ True（bool）/ None
        for bad in (-1, 1.5, "10", True, None):
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    estimate(bad, "gpt-4o")

    def test_token_estimate_is_frozen(self) -> None:
        result = estimate(1, "gpt-4o")
        with self.assertRaises(FrozenInstanceError):
            result.input_tokens = 99  # type: ignore[misc]

    def test_token_estimate_dataclass_fields(self) -> None:
        # 字段顺序冻结。
        self.assertEqual(
            [f.name for f in TokenEstimate.__dataclass_fields__.values()],
            [
                "input_tokens",
                "output_tokens",
                "total_tokens",
                "truth_level",
                "tokenizer_model",
                "revision",
                "estimated",
            ],
        )


class RevisionTests(unittest.TestCase):
    """§5.3 tokenizer-model revision 绑定。"""

    def test_known_prefixes(self) -> None:
        cases = {
            "gpt-4o": ("o200k_base", "2024-05-13"),
            "gpt-4o-mini": ("o200k_base", "2024-05-13"),
            "gpt-4": ("cl100k_base", "2023-06-13"),
            "gpt-4-turbo": ("cl100k_base", "2023-06-13"),
            "gpt-3.5-turbo": ("cl100k_base", "2023-06-13"),
            "claude-3-5-sonnet": ("anthropic-claude", "2024-06-20"),
            "gemini-1.5-pro": ("sentencepiece-gemini", "2024-09-24"),
        }
        for model, (tokenizer_model, revision) in cases.items():
            with self.subTest(model=model):
                binding = bind_revision(model)
                self.assertEqual(binding.model, model)
                self.assertIs(binding.bound, True)
                self.assertEqual(binding.reason, "known_model")
                self.assertEqual(binding.tokenizer_model, tokenizer_model)
                self.assertEqual(binding.revision, revision)

    def test_gpt4o_precedes_gpt4(self) -> None:
        # 更具体的 gpt-4o 必须先于 gpt-4 匹配。
        self.assertEqual(bind_revision("gpt-4o").tokenizer_model, "o200k_base")
        self.assertEqual(bind_revision("gpt-4-turbo").tokenizer_model, "cl100k_base")

    def test_known_model_explicit_tokenizer_differing_still_bound(self) -> None:
        binding = bind_revision("gpt-4o", tokenizer_model="bogus-custom")
        self.assertIs(binding.bound, True)
        self.assertEqual(binding.reason, "known_model")
        self.assertEqual(binding.tokenizer_model, "o200k_base")
        self.assertEqual(binding.revision, "2024-05-13")

    def test_unknown_model_no_tokenizer(self) -> None:
        binding = bind_revision("llama-3.1-70b")
        self.assertIs(binding.bound, False)
        self.assertEqual(binding.reason, "unknown_model")
        self.assertEqual(binding.tokenizer_model, "unknown")
        self.assertEqual(binding.revision, "unknown")
        self.assertEqual(binding.model, "llama-3.1-70b")

    def test_unknown_model_explicit_unverified(self) -> None:
        binding = bind_revision("llama-3.1-70b", tokenizer_model="my-tokenizer")
        self.assertIs(binding.bound, False)
        self.assertEqual(binding.reason, "explicit_unverified")
        self.assertEqual(binding.tokenizer_model, "my-tokenizer")
        self.assertEqual(binding.revision, "unknown")

    def test_never_raise_on_odd_input_coerced_to_str(self) -> None:
        for odd in (None, 123, 4.5, True):
            with self.subTest(odd=odd):
                binding = bind_revision(odd)
                self.assertIsInstance(binding.model, str)
                self.assertIs(binding.bound, False)

    def test_to_json_exact_keys_and_order(self) -> None:
        binding = bind_revision("gpt-4o")
        self.assertEqual(
            list(binding.to_json().keys()),
            ["model", "tokenizerModel", "revision", "bound", "reason"],
        )
        self.assertEqual(
            binding.to_json(),
            {
                "model": "gpt-4o",
                "tokenizerModel": "o200k_base",
                "revision": "2024-05-13",
                "bound": True,
                "reason": "known_model",
            },
        )

    def test_revision_binding_is_frozen(self) -> None:
        binding = bind_revision("gpt-4o")
        with self.assertRaises(FrozenInstanceError):
            binding.model = "x"  # type: ignore[misc]

    def test_revision_binding_dataclass_fields(self) -> None:
        self.assertEqual(
            [f.name for f in RevisionBinding.__dataclass_fields__.values()],
            ["model", "tokenizer_model", "revision", "bound", "reason"],
        )


if __name__ == "__main__":
    unittest.main()
