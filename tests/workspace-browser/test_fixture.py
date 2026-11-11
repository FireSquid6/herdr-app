"""HTTP fixture tests, NOT evidence of native browser or file transfer support."""

import hashlib
import http.client
import json
import threading
import unittest

from server import DOWNLOAD, FixtureServer, MAX_UPLOAD


class FixtureTests(unittest.TestCase):
    def setUp(self):
        self.server = FixtureServer()
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()

    def request(self, method, path, body=None, headers=None):
        conn = http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=5)
        try:
            conn.request(method, path, body, headers or {})
            response = conn.getresponse()
            return response.status, dict(response.getheaders()), response.read()
        finally:
            conn.close()

    def test_serves_real_page_and_original_popup(self):
        status, headers, body = self.request("GET", "/")
        self.assertEqual(status, 200)
        self.assertIn("text/html", headers["Content-Type"])
        self.assertIn(b"debugger;", body)
        self.assertIn(b"window.fixtureMemory = crypto.randomUUID()", body)
        self.assertIn(b"window.opener.postMessage", self.request("GET", "/popup")[2])

    def test_http_requests_are_observed_not_inferred(self):
        self.assertEqual(json.loads(self.request("GET", "/observed")[2])["requests"], 0)
        self.request("GET", "/request")
        self.assertEqual(json.loads(self.request("GET", "/observed")[2])["requests"], 1)

    def test_upload_digest_and_download_content(self):
        data = b"synthetic upload\n"
        status, _, body = self.request("POST", "/upload", data, {"X-Fixture-Name": "synthetic.txt"})
        self.assertEqual(status, 200)
        record = json.loads(body)
        self.assertEqual(record, {"name": "synthetic.txt", "size": len(data), "sha256": hashlib.sha256(data).hexdigest()})
        self.assertEqual(json.loads(self.request("GET", "/observed")[2])["uploads"], [record])
        status, headers, body = self.request("GET", "/download")
        self.assertEqual((status, body), (200, DOWNLOAD))
        self.assertIn("attachment", headers["Content-Disposition"])

    def test_oversized_upload_rejected_without_body_allocation(self):
        status, _, _ = self.request("POST", "/upload", b"", {"Content-Length": str(MAX_UPLOAD + 1)})
        self.assertEqual(status, 413)
        self.assertEqual(json.loads(self.request("GET", "/observed")[2])["uploads"], [])

    def test_cookie_is_only_synthetic_and_paths_are_not_files(self):
        self.assertIn("fixture=workspace-test", self.request("GET", "/cookie")[1]["Set-Cookie"])
        self.assertEqual(self.request("GET", "/../../etc/passwd")[0], 404)


if __name__ == "__main__":
    unittest.main()
