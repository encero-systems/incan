# The pinned Contracts graph fixture: an ordinary codegraph export and query, and one LSP navigation session.
require_relative 'lsp_client'
require_relative 'session'

module HeadToHead
  module GraphLsp
    FIXTURE = 'workspaces/contracts/fixtures/graph/incan/relationships'.freeze
    CONTRACT_CASE = 'graph/incan/relationships'.freeze
    EXPORT = %w[inspect codegraph . --format jsonl].freeze
    QUERY = %w[inspect codegraph src/main.incn --query defs helper].freeze
    QUERY_LINE = 'declaration provider::helper src/provider.incn:1'.freeze
    LSP_TIMEOUT_SECONDS = 900

    # Each engine gets its own copy of the fixture and Bakery, prepared as the LSP contract prepares it (untimed).
    def self.setup(session)
      ENGINES.to_h do |engine|
        root = session.directory('graph-lsp', engine)
        project = File.join(root, 'project')
        FileUtils.cp_r(File.join(session.source, FIXTURE), project)
        bakery = HeadToHead.clone_tree(session.prefill, File.join(root, 'bakery'))
        arm = { engine: engine, root: root, project: project, bakery: bakery,
                environment: session.environment(engine, bakery) }
        scope = { arm: 'graph-lsp', engine: engine, state: 'setup' }
        prepared = session.sample(scope.merge(role: 'prepare'), project, arm.fetch(:environment),
                                  [session.launcher, 'build', 'src/main.incn', '--profile', 'release'], timed: false)
        HeadToHead.require_fact(session.succeeded?(prepared), "cannot prepare the graph fixture for #{engine}", prepared)
        warm = session.sample(scope.merge(role: 'warmup-export'), project, arm.fetch(:environment),
                              [session.launcher, *EXPORT], timed: false)
        HeadToHead.require_fact(session.succeeded?(warm), "first graph export failed for #{engine}", warm)
        contract(session, arm) if session.settings[:contracts_runner]
        [engine, arm]
      end
    end

    # The stock Contracts assertions for this case, including the live LSP contract, under one engine's selector.
    def self.contract(session, arm)
      scratch = HeadToHead.make_directory(File.join(arm.fetch(:root), 'contract-scratch'))
      environment = arm.fetch(:environment).merge('INCAN_LSP' => session.lsp, 'INCAN_STDLIB' => '', 'RUSTC_BOOTSTRAP' => nil)
      argv = [session.settings.fetch(:contracts_runner), session.launcher, File.join(session.source, 'workspaces/contracts/fixtures'),
              '--scratch', scratch, '--jobs', '2', CONTRACT_CASE]
      entry = session.sample({ arm: 'graph-lsp', engine: arm.fetch(:engine), state: 'setup', role: 'contract-case' },
                             session.source, environment, argv, timed: false)
      passed = session.succeeded?(entry) && entry.fetch(:raw_stdout).lines.any? { |line| line.strip == "PASS #{CONTRACT_CASE}" }
      HeadToHead.require_fact(passed, "the stock #{CONTRACT_CASE} case did not pass under #{arm.fetch(:engine)}", entry)
    end

    def self.roots(session, arm)
      { arm.fetch(:project) => '$PROJECT', arm.fetch(:bakery) => '$BAKERY', arm.fetch(:root) => '$ARM', session.home => '$HOME' }
    end

    def self.run_pair(session, pair, arms, references)
      session.order(pair).each_with_index do |engine, position|
        arm = arms.fetch(engine)
        scope = { arm: 'graph-lsp', pair: pair, position: position, engine: engine, state: 'warm-unchanged' }
        graph_request(session, arm, scope.merge(stage: 'graph-export'), EXPORT, references, nil)
        graph_request(session, arm, scope.merge(stage: 'graph-query'), QUERY, references, QUERY_LINE)
        lsp_session(session, arm, scope.merge(stage: 'lsp-navigation'), references)
      end
    end

    # One ordinary graph request; its normalized reply must equal the first reply either engine gave.
    def self.graph_request(session, arm, scope, arguments, references, required_line)
      entry = session.sample(scope.merge(role: 'ordinary'), arm.fetch(:project), arm.fetch(:environment),
                             [session.launcher, *arguments])
      HeadToHead.require_fact(session.succeeded?(entry), "#{scope.fetch(:stage)} failed for #{arm.fetch(:engine)}", entry)
      reply = HeadToHead.normalize(entry.fetch(:raw_stdout).dup.force_encoding(Encoding::UTF_8), roots(session, arm))
      if required_line
        HeadToHead.require_fact(reply.lines.any? { |line| line.include?(required_line) }, "#{scope.fetch(:stage)} lost #{required_line}", entry)
      end
      reply = stable_snapshot(session, scope, reply, references, entry) if arguments == EXPORT
      compare(session, scope, reply, references, entry)
    end

    # The header's `snapshot_id` names the arm's own path and store, so engines compare with it masked. Within one
    # engine an unchanged request on unchanged inputs must keep it.
    def self.stable_snapshot(session, scope, reply, references, entry)
      snapshot = JSON.parse(reply.lines.first).fetch('snapshot_id')
      first = (references["snapshot:#{scope.fetch(:engine)}"] ||= snapshot)
      session.log(scope.merge(role: 'snapshot-stability', snapshot_id: snapshot, first_snapshot_id: first, equal: snapshot == first))
      HeadToHead.require_fact(snapshot == first, 'an unchanged graph export changed its snapshot identity', entry)
      reply.gsub(snapshot, '$SNAPSHOT')
    end

    def self.compare(session, scope, reply, references, entry)
      digest = Digest::SHA256.hexdigest(reply)
      reference = (references[scope.fetch(:stage)] ||= { digest: digest, engine: scope.fetch(:engine), pair: scope[:pair] })
      session.log(scope.merge(role: 'reply-parity', normalized_sha256: digest, reference: reference,
                              equal: digest == reference.fetch(:digest)))
      HeadToHead.require_fact(digest == reference.fetch(:digest), "#{scope.fetch(:stage)} reply differs from the reference", entry)
    end

    def self.cursor(uri, source, fragment, name)
      line = source.lines.index { |text| text.include?(fragment) }
      HeadToHead.refuse("fixture has no #{fragment}") unless line
      { textDocument: { uri: uri }, position: { line: line, character: source.lines[line].index(name) } }
    end

    # Open the fixture, wait for its version-one diagnostics, navigate to the provider declaration and shut down.
    def self.lsp_session(session, arm, scope, references)
      project = arm.fetch(:project)
      path = File.join(project, 'src/main.incn')
      source = File.read(path)
      uri = "file://#{path}"
      stderr = File.join(session.directory('graph-lsp', arm.fetch(:engine), 'lsp-logs'),
                         format('pair-%02d.stderr', scope.fetch(:pair)))
      before = HeadToHead.host_sample(session.slot_root) if session.timed
      client = LspClient.new([session.lsp, '--profile', 'release'], arm.fetch(:environment), project, stderr, LSP_TIMEOUT_SECONDS)
      replies = navigate(client, project, uri, source)
      status = client.finish
      client.mark('exited')
      raw = stderr.sub(/\.stderr\z/, '.replies.json')
      File.write(raw, JSON.pretty_generate({ uri: uri, document_version: 1 }.merge(replies)) + "\n")
      entry = scope.merge(role: 'ordinary', argv: [session.lsp, '--profile', 'release'], cwd: project,
                          environment: arm.fetch(:environment), exit: status.exitstatus, signal: status.termsig,
                          stderr_log: stderr, replies: raw, replies_sha256: HeadToHead.sha256(raw), timed: session.timed)
      entry.merge!(marks_ms: client.marks, host_before: before, host_after: HeadToHead.host_sample(session.slot_root)) if session.timed
      session.log(entry)
      HeadToHead.require_fact(status.success?, "LSP did not exit cleanly for #{arm.fetch(:engine)}", entry)
      compared = replies.slice(:diagnostics, :definition).merge(references: replies.fetch(:references).sort_by(&:to_s))
      compare(session, scope, HeadToHead.normalize(JSON.generate(compared), roots(session, arm)), references, entry)
    end

    def self.navigate(client, project, uri, source)
      capabilities = client.request('initialize', { processId: nil, rootUri: "file://#{project}", capabilities: {} })
      client.mark('initialized')
      HeadToHead.require_fact(capabilities.to_s.include?('callHierarchyProvider'), 'LSP did not advertise call hierarchy')
      client.notify('initialized', {})
      document = { uri: uri, languageId: 'incan', version: 1, text: source }
      client.notify('textDocument/didOpen', { textDocument: document })
      diagnostics = client.diagnostics(uri, 1)
      client.mark('diagnostics')
      errors = diagnostics.select { |item| item['severity'] == 1 }
      HeadToHead.require_fact(errors.empty?, 'LSP analysis refused the fixture', errors)
      at_helper = cursor(uri, source, 'println(helper())', 'helper')
      definition = client.request('textDocument/definition', at_helper)
      client.mark('definition')
      HeadToHead.require_fact(definition.to_s.include?('/src/provider.incn'), 'LSP definition missed the provider', definition)
      references = client.request('textDocument/references', at_helper.merge(context: { includeDeclaration: true }))
      client.mark('references')
      HeadToHead.require_fact(references.is_a?(Array) && references.size >= 2, 'LSP references omitted a site', references)
      client.request('shutdown', nil)
      client.notify('exit', nil)
      # The server's own reply bodies, kept as evidence; comparison sorts the references.
      { initialize: capabilities, diagnostics: diagnostics, definition: definition, references: references }
    end
  end
end
