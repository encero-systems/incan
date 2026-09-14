# Triage contract

An issue is not finished when its body is written. It is finished when someone else can act on it without first sorting it. Six things make that true, and all six are set **at creation** — not later, not in a sweep.

## 1. It follows a template

Use one of `bug_report.yml`, `chore.yml`, `feature_request.yml`, `documentation.yml`, `rfc_proposal.yml`. The title carries the matching prefix: `bug - `, `chore - `, `feature - `, `RFC proposal - `.

`gh issue create` applies no template. Passing `--body` bypasses them entirely, so fill the template's sections by hand when creating from the CLI.

## 2. It is on the project board

Project 2. An issue that is not on the board does not exist for planning purposes, whatever its milestone says.

## 3. It has a board status

Backlog unless work has actually started. Status is how the board reports progress; an unset status reports nothing.

## 4. It is in a milestone

A milestone is a release boundary, not a folder. The question is not "is this related to 0.6" but "must this ship before 0.6 ships". If the answer is no, it belongs in a later milestone or in none.

## 5. It has a type

Bug, Chore, or Feature. Derivable from the title prefix; there is no reason for it to be unset.

## 6. It has a parent, and a real reason to exist

The parent is what parks the issue in a slice, so the work can be read as belonging somewhere. Walking down from the execution ledger must reach it.

And the reason: an issue answers something tangible to a real audience. It earns its place when at least one holds.

- A **user** can hit it.
- It needs a decision its author cannot make.
- It must outlive the current slice under someone else's ownership.

## What is not an issue

Measurements, risk notes, withdrawn attempts, progress updates, and anything the author is about to fix themselves. These are working notes. They belong in the author's own ledger, not on a board someone else has to sweep.

The cost of an issue is not writing it. It is the triage it forces on whoever owns the board — so an issue that cannot be triaged at creation does not get created.
