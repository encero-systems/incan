# Store import profile

Status: draft. Profile ID `store-import/1`. Encoding, decoding and parsing work without a store. This profile covers what a store adds when it loads a COMIC document or a set of Crumbs: crumbling. CrumbDB is the reference store.

## Crumbling

To crumble a document is to load its values into a store, each under a namespace chosen at load time. Publishing `shop` as `acme/shop` does not rewrite the document: `shop.billing.Invoice` crumbled into `acme` is found under `acme.shop.billing.Invoice`.

A load must:

- publish the whole document as one transaction, or none of it;
- refuse a value whose `___object_hash` does not match its contents;
- refuse a value whose `___model_hash` differs from its family's model, unless an explicit migration or compatible description applies;
- resolve each reference against the store and the rest of the document, so forward references work, and refuse one that finds nothing in either;
- keep or remap `___id` and `___revision` explicitly;
- give a value without `___id` an identity derived from the load's import key, its type, its canonical projection and its position among equal values, so crumbling the same document twice changes nothing and two identical rows stay two records;
- refuse to write outside the namespace it loads into, so a document cannot claim `std` or another Loaf;
- load a secret-typed field withheld, so the store holds the marker and never the contents, and refuse every other marker;
- leave a document's namespace remapping unable to change a value's nominal identity.

A binding loaded this way is a queryable description. A program cannot import it as a type, which still takes a declaration in source. Writing a document out of a store is an export, and follows the store's declared privacy and retention profiles.

An export written as Crumbs is a sealed set, and loading a sealed set checks its seal before anything is published. A COMIC text document is never sealed: it loads whole when every entry is closed, so a caller that must know an export arrived complete asks for Crumbs.

## Open

- **Families.** A store keeps records in families connected by relationships, so a namespace has to land on those. A path cannot simply name a family, because moving a type or publishing a Loaf must not rename one. Crumbling needs a declared, stable mapping from a type to its family, and a binding's left side becomes a relationship.
- **Identity text.** The text form of `___id` and `___revision`.
