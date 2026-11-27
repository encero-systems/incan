# Two installed homes per engine over that engine's own shared Bakery: distinct views, shared results, follow-up work.
require_relative 'hello'
require_relative 'pins'

module HeadToHead
  module TwoHomes
    NAMES = %w[a b].freeze
    # Home, stage and state of each timed ordinary request, in order.
    STEPS = [%w[a first-home two-home-first], %w[b second-home two-home-shared],
             %w[a unchanged-first two-home-unchanged], %w[b unchanged-second two-home-unchanged]].freeze
    def self.home_environment(home, engine)
      launcher = File.join(home, 'bin/incan')
      { 'INCAN_HOME' => home, 'INCAN' => launcher, 'OVEN_ENGINE_BINARY' => launcher,
        'INCAN_LSP' => File.join(home, 'bin/incan-lsp'), 'INCAN_BAKERY' => nil, 'INCAN_ENGINE' => engine }
    end

    # Short-lived homes installed from the same archive through the ordinary command, outside the timed window.
    def self.install(session)
      root = session.directory('two-homes', 'homes')
      primary = Pins.installation(session.home)
      ENGINES.to_h do |engine|
        homes = NAMES.to_h { |name| [name, install_home(session, File.join(root, "#{engine}-#{name}"), primary)] }
        [engine, homes]
      end
    end

    def self.install_home(session, home, primary)
      bin = HeadToHead.make_directory(File.join(home, 'bin'))
      %w[incan incan-lsp].each { |name| FileUtils.cp(File.join(session.home, 'bin', name), bin, preserve: true) }
      File.symlink('incan', File.join(bin, 'oven'))
      entry = session.sample({ arm: 'two-homes', role: 'home-install', home: home }, home,
                             home_environment(home, nil).merge('INCAN_ENGINE' => nil),
                             [File.join(bin, 'incan'), 'toolchain', 'install', session.settings.fetch(:archive)],
                             timed: false)
      HeadToHead.require_fact(session.succeeded?(entry), "cannot install secondary home #{home}", entry)
      installed = Pins.installation(home)
      %i[launcher_sha256 lsp_sha256 manifest_sha256 driver_sha256 compiler_records].each do |key|
        HeadToHead.require_fact(installed.fetch(key) == primary.fetch(key), "secondary home #{key} differs", installed)
      end
      Hello.prime(session, home, File.basename(home), home_environment(home, nil))
      session.log({ arm: 'two-homes', role: 'home-catalog', home: home, stage: 'primed',
                    catalog_sha256: Pins.inventory_digest(File.join(home, 'bakery')) })
      home
    end

    # Bind a home to a Bakery through its `[storage] root`; a previous pair's descriptor is withdrawn first.
    def self.bind(home, bakery)
      File.write(File.join(home, 'config.toml'), "[storage]\nroot = #{JSON.generate(bakery)}\n")
      withdrawn = %w[bakery.view bakery.view.crumb].select { |name| File.exist?(File.join(home, name)) }
      withdrawn.each { |name| File.delete(File.join(home, name)) }
      withdrawn
    end

    def self.run_pair(session, pair, homes)
      root = session.directory('two-homes', format('pair-%02d', pair))
      session.order(pair).each_with_index do |engine, position|
        arm = File.join(root, engine)
        bakery = HeadToHead.clone_tree(session.prefill, File.join(arm, 'bakery'))
        withdrawn = homes.fetch(engine).transform_values { |home| bind(home, bakery) }
        projects = NAMES.to_h do |name|
          project = HeadToHead.make_directory(File.join(arm, "project-#{name}"))
          File.binwrite(File.join(project, 'hello.incn'), Hello.stage_source('cold').first)
          [name, project]
        end
        stopped = HeadToHead.quiesce(homes.fetch(engine).values + [session.home])
        context = { pair: pair, position: position, engine: engine, bakery: bakery, withdrawn: withdrawn,
                    workers_stopped_before: stopped }
        STEPS.each { |name, stage, state| step(session, context, homes.fetch(engine).fetch(name), projects.fetch(name), stage, state) }
        views(session, context, homes.fetch(engine))
        homes.fetch(engine).each_value do |home|
          session.log({ arm: 'two-homes', role: 'home-catalog', home: home, pair: pair, engine: engine, stage: 'after-pair',
                        catalog_sha256: Pins.inventory_digest(File.join(home, 'bakery')) })
        end
        release(session, context, homes.fetch(engine))
      end
      session.retire(root)
    end

    # Stop both homes' workers, then withdraw each home's view descriptor and storage root, so no home stays bound
    # to this pair's Bakery when it is retired. Oven has no unbind command; the descriptors' contents are already in
    # the view-isolation sample.
    def self.release(session, context, homes)
      stopped = HeadToHead.quiesce(homes.values)
      released = homes.transform_values do |home|
        names = %w[bakery.view bakery.view.crumb config.toml].select { |name| File.exist?(File.join(home, name)) }
        names.each { |name| File.delete(File.join(home, name)) }
        names
      end
      bound = homes.values.select { |home| File.exist?(File.join(home, 'bakery.view')) || File.exist?(File.join(home, 'config.toml')) }
      entry = { arm: 'two-homes', role: 'binding-release', pair: context.fetch(:pair), engine: context.fetch(:engine),
                bakery: context.fetch(:bakery), workers_stopped: stopped, released: released, still_bound: bound }
      session.log(entry)
      HeadToHead.require_fact(bound.empty? && HeadToHead.home_workers(homes.values).empty?,
                              'a home stayed bound to the pair Bakery', entry)
    end

    # The short-lived homes go once the arm has passed; their install and catalog evidence stays in the samples.
    def self.retire_homes(session, homes)
      return unless session.settings[:retire_trees]

      paths = homes.values.flat_map(&:values)
      HeadToHead.quiesce(paths)
      paths.each { |home| FileUtils.rm_rf(home) }
      session.log({ arm: 'two-homes', role: 'retired-homes', homes: paths })
    end

    def self.step(session, context, home, project, stage, state)
      before = HeadToHead.store_counts(context.fetch(:bakery))
      scope = context.merge(arm: 'two-homes', stage: stage, state: state, role: 'ordinary', home: home)
      entry = session.sample(scope, project, home_environment(home, context.fetch(:engine)),
                             [File.join(home, 'bin/incan'), 'run', 'hello.incn'])
      Hello.check_output(entry, Hello::INITIAL, "#{stage} #{context.fetch(:engine)}")
      binary = File.join(project, 'target/incan/debug/bin/hello')
      HeadToHead.require_fact(File.executable?(binary), "#{stage} produced no native executable", entry)
      after = HeadToHead.store_counts(context.fetch(:bakery))
      session.log(scope.merge(role: 'store-growth', artifact_sha256: HeadToHead.sha256(binary),
                              growth: after.to_h { |part, count| [part, count - before.fetch(part)] }))
    end

    # One store identity and locator, two distinct home views, and no ambient override binding (BE-01).
    def self.views(session, context, homes)
      descriptors = homes.transform_values { |home| descriptor(home) }
      store_ids = descriptors.values.map { |view| view['store_id'] }
      locators = descriptors.values.map { |view| view['locator'] }
      home_views = descriptors.values.map { |view| view['home_view'] }
      bakery_paths = [context.fetch(:bakery), File.realpath(context.fetch(:bakery))].uniq
      isolated = store_ids.uniq.size == 1 && !store_ids.first.nil? && locators.uniq.size == 1 &&
                 bakery_paths.include?(locators.first) && home_views.compact.uniq.size == homes.size
      entry = { arm: 'two-homes', pair: context.fetch(:pair), engine: context.fetch(:engine), role: 'view-isolation',
                descriptors: descriptors, one_store: store_ids.uniq.size == 1, distinct_views: home_views.uniq.size == 2,
                locators: locators.uniq, isolated: isolated }
      session.log(entry)
      HeadToHead.require_fact(isolated, 'two homes did not bind one store under two distinct views', entry)
    end

    def self.descriptor(home)
      path = File.join(home, 'bakery.view')
      HeadToHead.require_fact(File.file?(path), "home #{home} wrote no bakery.view")
      text = File.read(path)
      JSON.parse(text)
    rescue JSON::ParserError
      HeadToHead.refuse("home #{home} wrote an unreadable bakery.view", { text: text })
    end
  end
end
