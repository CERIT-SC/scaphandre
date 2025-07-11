use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};
use nvml_wrapper::error::NvmlError;
use nvml_wrapper::struct_wrappers::device::ProcessUtilizationSample;
use nvml_wrapper::{cuda_driver_version_major, cuda_driver_version_minor, Nvml, Device};
use std::error::Error;
use crate::sensors::{Record, RecordStorage};
use crate::sensors::current_system_time_since_epoch;
use crate::sensors::units::Unit;

#[derive(Debug)]
pub struct NvidiaNVMLGpu {
    pub index: u32,
    pub vendor: String,
    pub model: String,
    pub arch: String,
    /// Measurements of power usage, stored as Record instances
    pub record_storage: RecordStorage,
    /// Outer vector stores history, inner vector stores samples for each process
    pub proc_utils: Vec<Vec<ProcessUtilizationSample>>,
}

impl NvidiaNVMLGpu {
    pub fn new(nvml: &Nvml, index: u32) -> Result<NvidiaNVMLGpu, Box<dyn Error>> {
        match nvml.device_by_index(index as u32) {
            Err(e) => Err(Box::new(e)),
            Ok(device) => {
                let model = device.name()?.to_string();
                let arch= device.architecture()?.to_string();

                Ok(NvidiaNVMLGpu {
                    index: index,
                    vendor: "nvidia".to_string(),
                    model: model,
                    arch: arch,
                    record_storage: RecordStorage::new(0),
                    proc_utils: vec![],
                })
            }
        }
    }

    /// Returns a new owned Vector being a clone of the current record_buffer.
    /// This does not affect the current buffer but is costly.
    pub fn get_records_passive(&self) -> Vec<Record> {
        let mut result = vec![];
        for r in &self.record_storage.records {
            result.push(Record::new(
                r.timestamp,
                r.value.clone(),
                Unit::MicroWatt,
            ));
        }
        result
    }

    pub fn get_pid_last_util(&self, pid: u32) -> Option<&ProcessUtilizationSample> {
        let last_proc_utils = self.proc_utils.last()?;
        for proc_util in last_proc_utils {
            if proc_util.pid == pid {
                return Some(proc_util);
            }
        }
        None
    }

    pub fn push_proc_utils(&mut self, proc_utils: Vec<ProcessUtilizationSample>) {
        self.proc_utils.push(proc_utils);
    }
}

#[derive(Debug)]
pub struct NvidiaNVML {
    pub nvml_obj: Nvml,
    pub gpus: Vec<NvidiaNVMLGpu>,
}

impl NvidiaNVML {
    pub fn new() -> Option<NvidiaNVML> {
        let nvml = match Nvml::init() {
            Ok(nvml) => nvml,
            Err(e) => {
                warn!("Failed to initialize NVML. There could be a problem with loading the NVML libraries. Error: {}", e);
                return None;
            }
        };

        let gpus_count = match nvml.device_count() {
            Ok(count) => count,
            Err(e) => {
                warn!("Failed to search Nvidia GPUs. Error: {}", e);
                return None;
            }
        };

        info!("Nvidia GPUs found! Count: {}", gpus_count);

        let mut gpus: Vec<NvidiaNVMLGpu> = vec![];
        for i in 0..gpus_count {
            match NvidiaNVMLGpu::new(&nvml, i) {
                Ok(gpu) => gpus.push(gpu),
                Err(e) => {
                    error!("Failed to initialize GPU handler. Index: {}. Error: {}", i, e);
                }
            }
        }

        Some(NvidiaNVML {
            nvml_obj: nvml,
            gpus: gpus,
        })
    }

    pub fn get_gpus_count(&self) -> usize {
        self.gpus.len()
    }

    fn get_device(&self, index: usize) -> Result<Device, Box<dyn Error>> {
        match self.nvml_obj.device_by_index(index as u32) {
            Err(e) => Err(Box::new(e)),
            Ok(device) => Ok(device),
        }
    }

    pub fn get_gpu_consumption(&self, index: usize) -> Result<Record, Box<dyn Error>> {
        let device = self.get_device(index)?;
        match device.power_usage() {
            // power_usage returns value in milli. We need micro.
            Ok(value) => Ok(Record::new(
                current_system_time_since_epoch(),
                (value * 1000).to_string(),
                Unit::MicroWatt
            )),
            Err(e) => Err(Box::new(e)),
        }
    }

    pub fn refresh_process_utilization(&mut self, index: usize) -> Result<(), Box<dyn Error>> {
        let device = self.get_device(index)?;
        let proc_utils = device.process_utilization_stats(None)?;
        self.gpus[index].push_proc_utils(proc_utils);
        Ok(())
    }

    pub fn refresh_records(&mut self) {
        for i in 0..self.get_gpus_count() {
            match self.get_gpu_consumption(i) {
                Ok(record) => self.gpus[i].record_storage.add_record(&record),
                Err(e) => {
                    warn!("Failed to refresh record for GPU with index {}.", i.to_string());
                }
            }
            self.refresh_process_utilization(i);
            info!("Refreshing records for GPU. Count of records: {}", self.gpus[i].record_storage.records.len());
        }
    }
}

impl Clone for NvidiaNVML {
    fn clone(&self) -> NvidiaNVML {
        // TODO dangerous
        NvidiaNVML::new().unwrap()
    }
}
