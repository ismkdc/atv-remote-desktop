"""Exercise the actual pyatv SRP handshake to catch dependency API drift."""
import binascii
import os
import unittest
from pyatv.auth.hap_srp import SRPAuthHandler
from pyatv.protocols.airplay.server_auth import generate_keys, new_server_session


class PairingTests(unittest.TestCase):
    def test_pin_handshake_produces_matching_keys_and_valid_proofs(self):
        server, salt = new_server_session(generate_keys(os.urandom(32)), '1234')
        client = SRPAuthHandler()
        client.initialize()
        client.step1('1234')
        public, proof = client.step2(binascii.unhexlify(server.public), binascii.unhexlify(salt))
        server.process(binascii.hexlify(public).decode(), salt)
        self.assertTrue(server.verify_proof(binascii.hexlify(proof)))
        self.assertEqual(client.shared_key, server.key)


if __name__ == '__main__':
    unittest.main()
