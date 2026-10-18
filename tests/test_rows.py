"""Tests for the row engine (pure Python)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "addon", "resources", "lib"))
import rows  # noqa: E402

MOVIES = [
    {"movieid": 1, "title": "Alien", "premiered": "1979-05-25", "file": "/m/alien.mkv"},
    {"movieid": 2, "title": "Heat", "year": 1995, "file": "/m/heat.mkv"},
    {"movieid": 3, "title": "Undated Movie", "file": "/m/u.mkv"},
]
SHOWS = [
    {"tvshowid": 10, "title": "Breaking Bad", "tag": [], "watchedepisodes": 1},
    {"tvshowid": 11, "title": "Some Channel", "tag": ["YouTube"], "watchedepisodes": 0},
    {"tvshowid": 12, "title": "Finished Show", "tag": [], "watchedepisodes": 2},
    {"tvshowid": 13, "title": "Empty Show", "tag": []},
]
EPISODES = [
    {"tvshowid": 10, "firstaired": "2013-09-29", "playcount": 0},
    {"tvshowid": 10, "firstaired": "2013-09-22", "playcount": 1},
    {"tvshowid": 11, "firstaired": "2024-06-02", "playcount": 0},
    {"tvshowid": 12, "firstaired": "1999-01-01", "playcount": 1},
    {"tvshowid": 12, "firstaired": "1998-01-01", "playcount": 1},
]
GAMES = [
    {"id": "g1", "title": "Zelda", "system": "nes", "released": "1987-01-01"},
    {"id": "g2", "title": "Sonic", "system": "genesis", "released": "1991-06-01"},
    {"id": "g3", "title": "No Date", "system": "nes", "released": None},
]


def catalog():
    return (rows.movie_items(MOVIES) + rows.show_items(SHOWS, EPISODES)
            + rows.game_items(GAMES, lambda g: "plugin://x/?mode=game&id=" + g["id"]))


def titles(row_id):
    return [i["title"] for i in rows.build_row(row_id, catalog())]


class RowsTest(unittest.TestCase):
    def test_everything_is_newest_first_across_kinds_undated_last(self):
        # Show/channel date = newest episode; movies and games = release date.
        order = titles("everything")
        self.assertEqual(order[:7], ["Some Channel", "Breaking Bad", "Finished Show", "Heat",
                                     "Sonic", "Zelda", "Alien"])
        self.assertEqual(order[7:], ["Empty Show", "No Date", "Undated Movie"])

    def test_youtube_tag_makes_a_channel(self):
        self.assertEqual(titles("channels"), ["Some Channel"])
        self.assertNotIn("Some Channel", titles("tvshows"))

    def test_movie_date_falls_back_to_year(self):
        self.assertEqual(titles("movies"), ["Heat", "Alien", "Undated Movie"])

    def test_unwatched_row_needs_progress_and_an_unwatched_episode(self):
        # Breaking Bad: 1 watched, newest episode unwatched. The channel has 0
        # watched; Finished Show has nothing unwatched.
        self.assertEqual(titles("unwatched"), ["Breaking Bad"])

    def test_unwatched_row_sorts_by_newest_unwatched_episode(self):
        eps = EPISODES + [{"tvshowid": 12, "firstaired": "2020-01-01", "playcount": 0}]
        items = rows.show_items(SHOWS, eps)
        got = [i["title"] for i in rows.build_row("unwatched", items)]
        self.assertEqual(got, ["Finished Show", "Breaking Bad"])

    def test_one_games_row_per_system_present(self):
        ids = [r[0] for r in rows.list_rows(catalog())]
        self.assertEqual(ids[-3:], ["games", "games:genesis", "games:nes"])
        self.assertEqual(titles("games:nes"), ["Zelda", "No Date"])

    def test_list_rows_hides_empty_rows_but_keeps_everything(self):
        only_movies = rows.movie_items(MOVIES)
        ids = [r[0] for r in rows.list_rows(only_movies)]
        self.assertEqual(ids, ["everything", "movies"])
        self.assertEqual([r[0] for r in rows.list_rows([])], ["everything"])

    def test_unknown_row(self):
        self.assertIsNone(rows.build_row("nope", catalog()))

    def test_show_item_links_to_library_and_game_to_launch_url(self):
        by = {i["title"]: i for i in catalog()}
        self.assertEqual(by["Breaking Bad"]["url"], "videodb://tvshows/titles/10/")
        self.assertTrue(by["Breaking Bad"]["is_folder"])
        self.assertEqual(by["Zelda"]["url"], "plugin://x/?mode=game&id=g1")
        self.assertFalse(by["Zelda"]["is_folder"])


if __name__ == "__main__":
    unittest.main()
