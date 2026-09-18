use std::{collections::HashMap, sync::Arc};

use parking_lot::RwLock;
use prometheus::{
  Opts,
  core::{Atomic, Collector, Desc, GenericCounter, Metric},
  proto::{MetricFamily, MetricType},
};

pub(crate) struct CounterFamily<Storage: Atomic> {
  options:    Opts,
  descriptor: Desc,
  counters:   Arc<RwLock<HashMap<Vec<String>, GenericCounter<Storage>>>>,
}

impl<Storage: Atomic> CounterFamily<Storage> {
  pub(crate) fn new(options: Opts, names: &[&str]) -> prometheus::Result<Self> {
    let descriptor = Desc::new(
      options.fq_name(),
      options.help.clone(),
      names.iter().map(|name| (*name).to_owned()).collect(),
      options.const_labels.clone(),
    )?;

    Ok(Self {
      options,
      descriptor,
      counters: Arc::new(RwLock::new(HashMap::new())),
    })
  }

  pub(crate) fn with_label_values(
    &self,
    values: Vec<String>,
  ) -> GenericCounter<Storage> {
    assert_eq!(values.len(), self.descriptor.variable_labels.len());

    if let Some(counter) = self.counters.read().get(values.as_slice()) {
      return counter.clone();
    }

    let mut counters = self.counters.write();
    if let Some(counter) = counters.get(values.as_slice()) {
      return counter.clone();
    }

    let mut options = self.options.clone();

    options.const_labels.extend(
      self
        .descriptor
        .variable_labels
        .iter()
        .cloned()
        .zip(values.iter().cloned()),
    );

    let counter = GenericCounter::with_opts(options)
      .expect("counter names and labels were validated at construction");

    counters.insert(values, counter.clone());
    counter
  }
}

impl<Storage: Atomic> Clone for CounterFamily<Storage> {
  fn clone(&self) -> Self {
    Self {
      options:    self.options.clone(),
      descriptor: self.descriptor.clone(),
      counters:   Arc::clone(&self.counters),
    }
  }
}

impl<Storage: Atomic> Collector for CounterFamily<Storage> {
  fn desc(&self) -> Vec<&Desc> {
    vec![&self.descriptor]
  }

  fn collect(&self) -> Vec<MetricFamily> {
    let mut family = MetricFamily::default();
    family.set_name(self.descriptor.fq_name.clone());
    family.set_help(self.descriptor.help.clone());
    family.set_field_type(MetricType::COUNTER);

    family
      .set_metric(self.counters.read().values().map(Metric::metric).collect());

    vec![family]
  }
}
