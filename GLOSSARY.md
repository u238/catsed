# catsed

A file transfer service that relays a file directly from one machine to another using only curl (or wget). Nothing is stored: bytes flow from Sender to Receiver while both are connected.

## Language

**Sender**:
The party offering a file, who opens a Transfer and receives its Transfer URL.
_Avoid_: Uploader, source, client

**Receiver**:
The single party who claims a Transfer by requesting its Transfer URL and receives the file.
_Avoid_: Downloader, recipient, target

**Transfer**:
One live relay of a file from a Sender to exactly one Receiver. It exists only while the Sender is connected and ends when the file is delivered, either side drops, or no Receiver claims it in time.
_Avoid_: Upload, session, share

**Transfer URL**:
The single-use URL, made of a Transfer Code plus the file's name, that the Sender hands to the Receiver. Both parts must match for the Claim to succeed.
_Avoid_: Link, share link, download URL

**Transfer Code**:
The short random identifier (e.g. `Ak9zL`) inside a Transfer URL that identifies one Transfer.
_Avoid_: Token, ID, key, slug

**Claim**:
The moment a Receiver connects to a Transfer URL and becomes that Transfer's only Receiver; afterwards the Transfer URL no longer works for anyone else. Only curl and wget can Claim; any other client sees the usage text and leaves the Transfer open.
_Avoid_: Redeem, consume, open
